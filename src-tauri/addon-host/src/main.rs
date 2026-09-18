//! One isolated JS runtime per supervised child; no Tauri or OS bindings in JS.
use codemux_addon_protocol::{
    limits,
    wire::{read_frame, Envelope},
    Manifest,
};
use rquickjs::{Context, Function, Runtime};
use serde_json::json;
use std::{
    collections::{HashSet, VecDeque},
    hash::{Hash, Hasher},
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

fn emit(value: &impl serde::Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "Serialization failed")?;
    if bytes.len() > limits::FRAME {
        return Err("Outgoing frame limit".into());
    }
    let mut output = io::stdout().lock();
    output
        .write_all(&bytes)
        .and_then(|_| output.write_all(b"\n"))
        .and_then(|_| output.flush())
        .map_err(|_| "Parent disconnected".into())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("Plugin host stopped: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let gone = Arc::new(AtomicBool::new(false));
    let input_gone = gone.clone();
    let (tx, rx) = mpsc::sync_channel(2);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        loop {
            match read_frame(&mut reader) {
                Ok(Some(line)) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        input_gone.store(true, Ordering::Release);
    });
    let first = rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Initialization deadline")?;
    let init = Envelope::parse(&first, None, false).map_err(|_| "Invalid initialization")?;
    if init.method.as_deref() != Some("initialize") {
        return Err("Expected initialize".into());
    }
    let generation = init.generation.clone();
    let params = init.params.ok_or("Missing initialization")?;
    if params["protocolVersion"] != 1 {
        return Err("Incompatible protocol".into());
    }
    let manifest = Manifest::parse(&serde_json::to_vec(&params["manifest"]).unwrap(), None)
        .map_err(|_| "Invalid manifest")?;
    // Source arrives in bounded initialize chunks so a 5 MiB author bundle never
    // requires relaxing the 1 MiB frame boundary.
    let mut source = String::new();
    let mut next = Some((init.id, params));
    loop {
        let (id, params) = next.take().unwrap();
        let chunk = params["source"].as_str().ok_or("Missing source chunk")?;
        if source.len() + chunk.len() > limits::BUNDLE {
            return Err("Bundle limit".into());
        }
        source.push_str(chunk);
        if let Some(id) = id {
            emit(&Envelope::response(
                &generation,
                id,
                Ok(json!({"accepted":true})),
            ))?;
        }
        if params["final"] == true {
            break;
        }
        let line = rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "Initialization deadline")?;
        let msg =
            Envelope::parse(&line, Some(&generation), false).map_err(|_| "Invalid source frame")?;
        if msg.method.as_deref() != Some("initialize") {
            return Err("Expected source chunk".into());
        }
        next = Some((msg.id, msg.params.unwrap()));
    }
    let rt = Runtime::new().map_err(|_| "Engine initialization failed")?;
    rt.set_memory_limit(limits::HEAP);
    rt.set_max_stack_size(limits::STACK);
    let deadline = Arc::new(Mutex::new(Instant::now() + Duration::from_secs(1)));
    let interrupt_deadline = deadline.clone();
    let interrupt_gone = gone.clone();
    rt.set_interrupt_handler(Some(Box::new(move || {
        interrupt_gone.load(Ordering::Acquire)
            || Instant::now() >= *interrupt_deadline.lock().unwrap()
    })));
    let rejected = Arc::new(Mutex::new(HashSet::new()));
    let tracker = rejected.clone();
    rt.set_host_promise_rejection_tracker(Some(Box::new(move |_, promise, _, handled| {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        promise.hash(&mut hash);
        let mut rejected = tracker.lock().unwrap();
        if handled {
            rejected.remove(&hash.finish());
        } else {
            rejected.insert(hash.finish());
        }
    })));
    let context = Context::full(&rt).map_err(|_| "Context initialization failed")?;
    let fault = Arc::new(AtomicBool::new(false));
    let output_fault = fault.clone();
    let output_generation = generation.clone();
    let mut requests = limits::RateLimit::default();
    let mut logs = limits::RateLimit::default();
    let mut patches = limits::RateLimit::default();
    let epoch = Instant::now();
    context
        .with(|ctx| -> rquickjs::Result<()> {
            ctx.globals().set(
                "__nativeNow",
                Function::new(ctx.clone(), move || epoch.elapsed().as_millis() as f64)?,
            )?;
            ctx.globals().set(
                "__nativeSend",
                Function::new(
                    ctx.clone(),
                    rquickjs::function::MutFn::new(move |line: String| {
                        let parsed =
                            Envelope::parse(line.as_bytes(), Some(&output_generation), true);
                        let Ok(mut msg) = parsed else {
                            output_fault.store(true, Ordering::Release);
                            return;
                        };
                        let now = Instant::now();
                        let allowed = match msg.method.as_deref() {
                            Some("host.request") => requests.accept(now, 20, 100),
                            Some("ui.patch") => patches.accept(now, 30, 1800),
                            Some("log") => {
                                if !logs.accept(now, 5, 300) {
                                    return;
                                }
                                let clean = msg
                                    .params
                                    .as_ref()
                                    .and_then(|p| p["message"].as_str())
                                    .unwrap_or("")
                                    .chars()
                                    .filter(|c| !c.is_control() || *c == '\n')
                                    .take(1024)
                                    .collect::<String>();
                                msg.params = Some(json!({"message":clean}));
                                true
                            }
                            _ => true,
                        };
                        if !allowed || emit(&msg).is_err() {
                            output_fault.store(true, Ordering::Release);
                        }
                    }),
                )?,
            )?;
            ctx.eval::<(), _>(include_str!("bootstrap.js"))?;
            ctx.globals()
                .get::<_, Function>("__configure")?
                .call::<_, ()>((
                    generation.clone(),
                    serde_json::to_string(&manifest).unwrap(),
                ))?;
            ctx.eval::<(), _>(source)?;
            Ok(())
        })
        .map_err(|_| "Plugin initialization failed")?;
    drain(&rt, &deadline)?;
    let mut cpu = VecDeque::<(Instant, Duration)>::new();
    loop {
        if gone.load(Ordering::Acquire) {
            return Ok(());
        }
        if fault.load(Ordering::Acquire) {
            return Err("Protocol or resource violation".into());
        }
        let delay = context
            .with(|ctx| {
                ctx.globals()
                    .get::<_, Function>("__nextWake")
                    .and_then(|f| f.call::<_, f64>(()))
            })
            .map_err(|_| "Runtime scheduler failed")?;
        let message = if delay < 0.0 {
            rx.recv().ok()
        } else {
            match rx.recv_timeout(Duration::from_millis(delay.max(1.0).min(30_000.0) as u64)) {
                Ok(v) => Some(v),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => return Ok(()),
            }
        };
        let started = Instant::now();
        while cpu
            .front()
            .is_some_and(|(time, _)| started.duration_since(*time) >= Duration::from_secs(5))
        {
            cpu.pop_front();
        }
        let used: Duration = cpu.iter().map(|(_, duration)| *duration).sum();
        if used >= Duration::from_secs(1) {
            return Err("Active CPU budget exceeded".into());
        }
        let parsed = message
            .as_ref()
            .map(|line| {
                Envelope::parse(line, Some(&generation), false)
                    .map_err(|_| "Invalid parent message")
            })
            .transpose()?;
        let activation = parsed
            .as_ref()
            .is_some_and(|m| m.method.as_deref() == Some("activate"));
        let budget = if activation {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(250)
        };
        *deadline.lock().unwrap() = started + budget.min(Duration::from_secs(1) - used);
        if let Some(msg) = &parsed {
            if let (Some(id), Some(_)) = (msg.id, &msg.method) {
                emit(&Envelope::response(
                    &generation,
                    id,
                    Ok(json!({"accepted":true})),
                ))?;
            }
        }
        context
            .with(|ctx| -> rquickjs::Result<()> {
                if let Some(msg) = &parsed {
                    ctx.globals()
                        .get::<_, Function>("__dispatch")?
                        .call::<_, ()>((serde_json::to_string(msg).unwrap(),))?;
                } else {
                    ctx.globals()
                        .get::<_, Function>("__tick")?
                        .call::<_, ()>(())?;
                }
                Ok(())
            })
            .map_err(|_| "Plugin callback failed")?;
        drain(&rt, &deadline)?;
        if !rejected.lock().unwrap().is_empty() {
            return Err("Unhandled plugin promise rejection".into());
        }
        cpu.push_back((started, started.elapsed()));
        if parsed
            .as_ref()
            .is_some_and(|m| m.method.as_deref() == Some("deactivate"))
        {
            return Ok(());
        }
        emit(
            &json!({"jsonrpc":"2.0","generation":generation,"method":"ready","params":{"phase":"yielded"}}),
        )?;
    }
}
fn drain(rt: &Runtime, deadline: &Mutex<Instant>) -> Result<(), String> {
    while rt.is_job_pending() {
        if Instant::now() >= *deadline.lock().unwrap() {
            return Err("Microtask deadline exceeded".into());
        }
        rt.execute_pending_job()
            .map_err(|_| "Plugin promise failed")?;
    }
    Ok(())
}
