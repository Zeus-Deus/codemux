//! One isolated JS runtime per supervised child; no Tauri or OS bindings in JS.
use codemux_addon_protocol::{
    limits,
    wire::{read_frame, Envelope},
    ErrorCode, Manifest, ProtocolError,
};
use rquickjs::{Context, Ctx, Exception, Function, Runtime};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet, VecDeque},
    hash::{Hash, Hasher},
    io::{self, Write},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

const LEVELS: [&str; 5] = ["log", "info", "warn", "error", "debug"];

fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>, &'static str> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| "Serialization failed")?;
    if bytes.len() > limits::FRAME {
        return Err("Outgoing frame limit");
    }
    bytes.push(b'\n');
    Ok(bytes)
}
fn write(bytes: &[u8]) -> Result<(), &'static str> {
    let mut output = io::stdout().lock();
    output
        .write_all(bytes)
        .and_then(|_| output.flush())
        .map_err(|_| "Parent disconnected")
}
fn emit(value: &impl serde::Serialize) -> Result<(), &'static str> {
    write(&encode(value)?)
}
/// Closes descriptors inherited from the app before any thread or plugin code
/// exists. Doing it here keeps the parent on its fast spawn path.
#[cfg(target_os = "linux")]
fn close_inherited() {
    // SAFETY: nothing in this process owns a descriptor above 2 yet.
    unsafe {
        if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) != 0 {
            // Kernels before 5.9: close a bounded descriptor range.
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            let end = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
                limit.rlim_cur.min(65536) as libc::c_int
            } else {
                65536
            };
            for fd in 3..end {
                libc::close(fd);
            }
        }
    }
}
fn main() {
    #[cfg(target_os = "linux")]
    close_inherited();
    // Every reason is a fixed host string; the supervisor accepts only known ones.
    if let Err(error) = run() {
        eprintln!("Plugin host stopped: {error}");
        std::process::exit(1);
    }
}
/// Rust owns timer limits, deadlines and wakeups. JS only maps due IDs to
/// callbacks, so replaced intrinsics never run outside an accounted window.
#[derive(Default)]
struct Timers {
    sequence: u64,
    live: BTreeMap<u64, (Instant, Duration, bool)>,
}
impl Timers {
    fn set(&mut self, delay: f64, repeat: bool) -> Option<u64> {
        if self.live.len() >= limits::TIMERS {
            return None;
        }
        // NaN and short delays become the 100 ms minimum.
        let delay = if delay >= 100.0 {
            delay.min(2_147_483_647.0)
        } else {
            100.0
        };
        let delay = Duration::from_millis(delay as u64);
        self.sequence += 1;
        self.live
            .insert(self.sequence, (Instant::now() + delay, delay, repeat));
        Some(self.sequence)
    }
    fn next(&self) -> Option<Instant> {
        self.live.values().map(|timer| timer.0).min()
    }
    fn due(&mut self, now: Instant) -> Vec<u64> {
        let mut due: Vec<_> = self
            .live
            .iter()
            .filter(|(_, timer)| timer.0 <= now)
            .map(|(id, timer)| (timer.0, *id))
            .collect();
        due.sort_unstable();
        for (_, id) in &due {
            let timer = self.live.get_mut(id).unwrap();
            if timer.2 {
                timer.0 = now + timer.1;
            } else {
                self.live.remove(id);
            }
        }
        due.into_iter().map(|(_, id)| id).collect()
    }
}
/// Child-to-parent traffic. Requests over the quota receive a bounded
/// RESOURCE_LIMIT response and UI batches over 30/s wait for the window instead
/// of stopping the plugin. Five violations within 10 s stop the generation.
struct Traffic {
    generation: String,
    requests: limits::RateLimit,
    logs: limits::RateLimit,
    patches: VecDeque<Instant>,
    held: VecDeque<Vec<u8>>,
    replies: VecDeque<Vec<u8>>,
    violations: VecDeque<Instant>,
    fault: Option<&'static str>,
}
impl Traffic {
    fn violation(&mut self, now: Instant) {
        while self
            .violations
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(10))
        {
            self.violations.pop_front();
        }
        self.violations.push_back(now);
        if self.violations.len() >= 5 {
            self.fault = Some("Repeated protocol or quota violations");
        }
    }
    fn write(&mut self, bytes: &[u8]) {
        if let Err(error) = write(bytes) {
            self.fault = Some(error);
        }
    }
    /// Earliest time the next UI batch may leave the 30/s and 1,800/min window.
    /// The guard keeps paced batches inside the parent's arrival-time window
    /// despite pipe and scheduling jitter.
    fn paced(&mut self, now: Instant) -> Instant {
        const GUARD: Duration = Duration::from_millis(50);
        while self
            .patches
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60) + GUARD)
        {
            self.patches.pop_front();
        }
        let mut at = now;
        for (count, window) in [(1800, 60), (30, 1)] {
            if self.patches.len() >= count {
                at = at.max(
                    self.patches[self.patches.len() - count] + Duration::from_secs(window) + GUARD,
                );
            }
        }
        at
    }
    fn release(&mut self, now: Instant) {
        while self.fault.is_none() && !self.held.is_empty() && self.paced(now) <= now {
            let bytes = self.held.pop_front().unwrap();
            self.patches.push_back(now);
            self.write(&bytes);
        }
    }
    fn release_at(&mut self, now: Instant) -> Option<Instant> {
        (!self.held.is_empty()).then(|| self.paced(now))
    }
    /// Answers a request with a bounded RESOURCE_LIMIT error and counts it.
    fn reject(&mut self, id: u64, message: &'static str, now: Instant) {
        let reply = Envelope::response(
            &self.generation,
            id,
            Err(ProtocolError::new(ErrorCode::ResourceLimit, message)),
        );
        self.replies.push_back(serde_json::to_vec(&reply).unwrap());
        self.violation(now);
    }
    /// Envelope::parse refuses a frame over 1 MiB unread, so classify it by
    /// its method alone. A request is answered; a UI batch or activation
    /// cannot be dropped, so it stops the generation.
    fn oversized(&mut self, line: &str, now: Instant) {
        #[derive(serde::Deserialize)]
        struct Head {
            method: Option<String>,
            id: Option<u64>,
        }
        match serde_json::from_str::<Head>(line) {
            Ok(Head {
                method: Some(method),
                id: Some(id),
            }) if method == "host.request" => self.reject(id, "Host request exceeds 1 MiB", now),
            Ok(Head {
                method: Some(method),
                ..
            }) if method == "ui.patch" || method == "ready" => {
                self.fault = Some("Outgoing frame limit")
            }
            _ => self.violation(now),
        }
    }
    fn send(&mut self, line: &str) {
        if self.fault.is_some() {
            return;
        }
        let now = Instant::now();
        if line.len() > limits::FRAME {
            return self.oversized(line, now);
        }
        let Ok(mut msg) = Envelope::parse(line.as_bytes(), Some(&self.generation), true) else {
            return self.violation(now);
        };
        match msg.method.as_deref() {
            Some("host.request") => {
                let Some(id) = msg.id else {
                    return self.violation(now);
                };
                if !self.requests.accept(now, 20, 100) {
                    return self.reject(id, "Plugin request quota exceeded", now);
                }
                match encode(&msg) {
                    Ok(bytes) => self.write(&bytes),
                    // Re-encoding can expand some numbers past the limit.
                    Err(_) => self.reject(id, "Host request exceeds 1 MiB", now),
                }
            }
            Some("ui.patch") => {
                // A patch cannot be dropped without corrupting the tree.
                let bytes = match encode(&msg) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        self.fault = Some(error);
                        return;
                    }
                };
                if self.held.is_empty() && self.paced(now) <= now {
                    self.patches.push_back(now);
                    self.write(&bytes);
                } else if self.held.len() < limits::VIEWS * 2 {
                    self.held.push_back(bytes);
                } else {
                    self.fault = Some("UI update queue overflow");
                }
            }
            Some("log") => {
                if !self.logs.accept(now, 5, 300) {
                    return;
                }
                let params = msg.params.as_ref();
                let clean = params
                    .and_then(|p| p["message"].as_str())
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n')
                    .take(1024)
                    .collect::<String>();
                let level = params
                    .and_then(|p| p["level"].as_str())
                    .filter(|level| LEVELS.contains(level))
                    .unwrap_or("log");
                msg.params = Some(json!({"message":clean,"level":level}));
                match encode(&msg) {
                    Ok(bytes) => self.write(&bytes),
                    Err(error) => self.fault = Some(error),
                }
            }
            _ => match encode(&msg) {
                Ok(bytes) => self.write(&bytes),
                Err(error) => self.fault = Some(error),
            },
        }
    }
}
fn run() -> Result<(), &'static str> {
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
        return Err("Expected initialize");
    }
    let generation = init.generation.clone();
    let params = init.params.ok_or("Missing initialization")?;
    if params["protocolVersion"] != 1 {
        return Err("Incompatible protocol");
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
            return Err("Bundle limit");
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
            return Err("Expected source chunk");
        }
        next = Some((msg.id, msg.params.unwrap()));
    }
    let rt = Runtime::new().map_err(|_| "Engine initialization failed")?;
    rt.set_memory_limit(limits::HEAP);
    rt.set_max_stack_size(limits::STACK);
    let deadline = Arc::new(Mutex::new(Instant::now() + Duration::from_secs(1)));
    let interrupt_deadline = deadline.clone();
    let interrupt_gone = gone.clone();
    let timed_out = Arc::new(AtomicBool::new(false));
    let interrupt_timed_out = timed_out.clone();
    rt.set_interrupt_handler(Some(Box::new(move || {
        let expired = Instant::now() >= *interrupt_deadline.lock().unwrap();
        if expired {
            interrupt_timed_out.store(true, Ordering::Release);
        }
        interrupt_gone.load(Ordering::Acquire) || expired
    })));
    let failure = |reason: &'static str| {
        if timed_out.load(Ordering::Acquire) {
            "Plugin CPU deadline exceeded"
        } else {
            reason
        }
    };
    let rejected = Arc::new(Mutex::new(HashSet::new()));
    let tracker = rejected.clone();
    let rejection_overflow = Arc::new(AtomicBool::new(false));
    let tracker_overflow = rejection_overflow.clone();
    rt.set_host_promise_rejection_tracker(Some(Box::new(move |_, promise, _, handled| {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        promise.hash(&mut hash);
        let mut rejected = tracker.lock().unwrap();
        if handled {
            rejected.remove(&hash.finish());
        } else if rejected.len() < 4096 {
            rejected.insert(hash.finish());
        } else {
            tracker_overflow.store(true, Ordering::Release);
        }
    })));
    let context = Context::full(&rt).map_err(|_| "Context initialization failed")?;
    let traffic = Rc::new(RefCell::new(Traffic {
        generation: generation.clone(),
        requests: limits::RateLimit::default(),
        logs: limits::RateLimit::default(),
        patches: VecDeque::new(),
        held: VecDeque::new(),
        replies: VecDeque::new(),
        violations: VecDeque::new(),
        fault: None,
    }));
    let timers = Rc::new(RefCell::new(Timers::default()));
    let epoch = Instant::now();
    // Module evaluation and activation share the single 1 s activation budget.
    let started = Instant::now();
    *deadline.lock().unwrap() = started + Duration::from_secs(1);
    let output = traffic.clone();
    let (set_timers, clear_timers) = (timers.clone(), timers.clone());
    context
        .with(|ctx| -> rquickjs::Result<()> {
            ctx.globals().set(
                "__nativeNow",
                Function::new(ctx.clone(), move || epoch.elapsed().as_millis() as f64)?,
            )?;
            ctx.globals().set(
                "__nativeSend",
                Function::new(ctx.clone(), move |line: String| {
                    output.borrow_mut().send(&line)
                })?,
            )?;
            ctx.globals().set(
                "__nativeTimer",
                Function::new(
                    ctx.clone(),
                    move |ctx: Ctx<'_>, delay: f64, repeat: bool| -> rquickjs::Result<f64> {
                        set_timers
                            .borrow_mut()
                            .set(delay, repeat)
                            .map(|id| id as f64)
                            .ok_or_else(|| Exception::throw_message(&ctx, "Timer limit"))
                    },
                )?,
            )?;
            ctx.globals().set(
                "__nativeClearTimer",
                Function::new(ctx.clone(), move |id: f64| {
                    clear_timers.borrow_mut().live.remove(&(id as u64));
                })?,
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
        .map_err(|_| failure("Plugin initialization failed"))?;
    drain(&rt, &deadline).map_err(failure)?;
    let mut cpu = VecDeque::<(Instant, Duration)>::from([(started, started.elapsed())]);
    let mut activation = Duration::from_secs(1).saturating_sub(started.elapsed());
    loop {
        if gone.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(error) = traffic.borrow().fault {
            return Err(error);
        }
        // Rust decides every wakeup: a queued quota reply, a parent message, a
        // due timer, or a deferred UI batch. No plugin JS runs to compute it.
        let now = Instant::now();
        let timer_at = timers.borrow().next();
        let reply = traffic.borrow_mut().replies.pop_front();
        let message = if reply.is_some() {
            reply
        } else if timer_at.is_some_and(|at| at <= now) {
            None
        } else {
            let release_at = traffic.borrow_mut().release_at(now);
            match timer_at.into_iter().chain(release_at).min() {
                None => match rx.recv() {
                    Ok(line) => Some(line),
                    Err(_) => return Ok(()),
                },
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(now)) {
                    Ok(line) => Some(line),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                },
            }
        };
        let started = Instant::now();
        traffic.borrow_mut().release(started);
        let due = if message.is_none() {
            timers.borrow_mut().due(started)
        } else {
            Vec::new()
        };
        if message.is_none() && due.is_empty() {
            continue;
        }
        while cpu
            .front()
            .is_some_and(|(time, _)| started.duration_since(*time) >= Duration::from_secs(5))
        {
            cpu.pop_front();
        }
        let used: Duration = cpu.iter().map(|(_, duration)| *duration).sum();
        if used >= Duration::from_secs(1) {
            return Err("Active CPU budget exceeded");
        }
        let parsed = message
            .as_ref()
            .map(|line| {
                Envelope::parse(line, Some(&generation), false)
                    .map_err(|_| "Invalid parent message")
            })
            .transpose()?;
        let budget = if parsed
            .as_ref()
            .is_some_and(|m| m.method.as_deref() == Some("activate"))
        {
            std::mem::take(&mut activation)
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
            context
                .with(|ctx| {
                    ctx.globals()
                        .get::<_, Function>("__dispatch")?
                        .call::<_, ()>((serde_json::to_string(msg).unwrap(),))
                })
                .map_err(|_| failure("Plugin callback failed"))?;
            drain(&rt, &deadline).map_err(failure)?;
        }
        // Due timers share this window's deadline; promise jobs run between them.
        for id in due {
            context
                .with(|ctx| {
                    ctx.globals()
                        .get::<_, Function>("__tick")?
                        .call::<_, ()>((id as f64,))
                })
                .map_err(|_| failure("Plugin callback failed"))?;
            drain(&rt, &deadline).map_err(failure)?;
        }
        if rejection_overflow.load(Ordering::Acquire) || !rejected.lock().unwrap().is_empty() {
            return Err("Unhandled plugin promise rejection");
        }
        cpu.push_back((started, started.elapsed()));
        if let Some(error) = traffic.borrow().fault {
            return Err(error);
        }
        traffic.borrow_mut().release(Instant::now());
        if parsed
            .as_ref()
            .is_some_and(|m| m.method.as_deref() == Some("deactivate"))
        {
            return Ok(());
        }
        // Only parent calls wait on this yield; timer ticks and replies send nothing.
        if let Some(id) = parsed.filter(|m| m.method.is_some()).and_then(|m| m.id) {
            emit(
                &json!({"jsonrpc":"2.0","generation":generation,"method":"ready","params":{"phase":"yielded","requestId":id}}),
            )?;
        }
    }
}
fn drain(rt: &Runtime, deadline: &Mutex<Instant>) -> Result<(), &'static str> {
    while rt.is_job_pending() {
        if Instant::now() >= *deadline.lock().unwrap() {
            return Err("Microtask deadline exceeded");
        }
        rt.execute_pending_job()
            .map_err(|_| "Plugin promise failed")?;
    }
    Ok(())
}
