use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
struct Host {
    child: Child,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<Value>,
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Host {
    fn new(source: &str) -> Self {
        Self::with(
            Command::new(env!("CARGO_BIN_EXE_codemux-addon-host")),
            source,
        )
    }
    fn with(mut command: Command, source: &str) -> Self {
        // Only the one-line stop reason reaches stderr, so it never fills.
        let mut child = command
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(64);
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        let mut host = Self {
            child,
            input,
            output: rx,
        };
        let manifest: Value =
            serde_json::from_str(include_str!("../../addon-protocol/fixtures/hello.json")).unwrap();
        host.send(
            "initialize",
            json!({"protocolVersion":1,"manifest":manifest,"source":source,"final":true}),
        );
        host
    }
    fn send(&mut self, method: &str, params: Value) {
        self.call(1, method, params);
    }
    fn call(&mut self, id: u64, method: &str, params: Value) {
        writeln!(self.input.as_mut().unwrap(),"{}",json!({"jsonrpc":"2.0","generation":"native-test","id":id,"method":method,"params":params})).unwrap();
    }
    fn receive(&self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(2))
            .expect("host reply deadline")
    }
    /// Frames until the deadline, or until the host closes its output.
    fn collect(&self, duration: Duration) -> Vec<Value> {
        let until = Instant::now() + duration;
        let mut frames = Vec::new();
        while let Ok(frame) = self
            .output
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            frames.push(frame);
        }
        frames
    }
    /// Waits for the yield of one parent call; false if the host exited first.
    fn yielded(&self, id: u64) -> bool {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            match self
                .output
                .recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                Ok(frame) if frame["method"] == "ready" && frame["params"]["requestId"] == id => {
                    return true
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("host yield deadline"),
            }
        }
    }
    fn alive(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
    fn stopped(&mut self) {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            assert!(Instant::now() < until, "host failed to stop");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// Waits for the exit and returns what the host printed on stderr.
    fn reason(&mut self) -> String {
        self.stopped();
        let mut text = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }
}
fn spin(milliseconds: u64) -> String {
    format!("{{const end=Date.now()+{milliseconds};while(Date.now()<end){{}}}}")
}
fn logs(frames: &[Value]) -> Vec<&Value> {
    frames
        .iter()
        .filter(|frame| frame["method"] == "log")
        .map(|frame| &frame["params"])
        .collect()
}
#[test]
fn native_runtime_has_no_ambient_authority_and_parent_eof_stops_it() {
    let mut host=Host::new("__codemuxRegister({}, ({send}) => () => send('log',{message:JSON.stringify([typeof process,typeof require,typeof fetch,typeof __TAURI_INTERNALS__])}));");
    assert_eq!(host.receive()["result"]["accepted"], true);
    host.send("activate", json!({}));
    host.receive();
    let log = host.receive();
    assert_eq!(
        log["params"]["message"],
        "[\"undefined\",\"undefined\",\"undefined\",\"undefined\"]"
    );
    host.input.take();
    host.stopped();
}
#[test]
fn hostile_callbacks_and_promise_jobs_are_bounded_in_real_processes() {
    for code in [
        "while(true){}",
        "function f(){f()} f()",
        "new ArrayBuffer(128*1024*1024)",
        "Promise.resolve().then(function loop(){Promise.resolve().then(loop)})",
        "throw Error('private source must not escape')",
    ] {
        let mut host = Host::new(&format!("__codemuxRegister({{}}, () => () => {{{code}}});"));
        host.receive();
        host.send("activate", json!({}));
        host.stopped();
    }
}
#[test]
fn ignored_shutdown_cannot_keep_the_native_runtime_alive() {
    for cleanup in [
        "while(true){}",
        "Promise.resolve().then(function loop(){Promise.resolve().then(loop)})",
        "throw Error('private cleanup failure')",
    ] {
        let mut host = Host::new(&format!(
            "__codemuxRegister({{}}, () => m => {{if(m.method==='deactivate'){{{cleanup}}}}});"
        ));
        host.receive();
        host.send("activate", json!({}));
        while host.receive()["method"] != "ready" {}
        let started = Instant::now();
        host.send("deactivate", json!({}));
        host.stopped();
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
#[test]
fn stale_generation_and_oversized_input_stop_only_the_child() {
    let mut host = Host::new("__codemuxRegister({},()=>()=>{});");
    host.receive();
    writeln!(
        host.input.as_mut().unwrap(),
        "{}",
        json!({"jsonrpc":"2.0","generation":"stale","id":2,"method":"activate","params":{}})
    )
    .unwrap();
    host.stopped();
    let mut host = Host::new("__codemuxRegister({},()=>()=>{});");
    host.receive();
    let _ = host
        .input
        .as_mut()
        .unwrap()
        .write_all(&vec![b'x'; 1024 * 1024 + 1]);
    host.stopped();
}
#[test]
fn module_evaluation_and_activation_share_one_cpu_second() {
    let started = Instant::now();
    let mut host = Host::new(&format!(
        "{}__codemuxRegister({{}}, ({{send}}) => m => {{if(m.method==='activate'){{{}send('ready',{{phase:'activated',registrations:[]}})}}}});",
        spin(800),
        spin(800)
    ));
    host.receive();
    host.send("activate", json!({}));
    let frames = host.collect(Duration::from_secs(2));
    host.stopped();
    // The host stops itself; it does not rely on the supervisor watchdog.
    assert!(started.elapsed() < Duration::from_millis(1400));
    assert!(!frames.iter().any(|f| f["params"]["phase"] == "activated"));
}
#[test]
fn scheduler_never_runs_replaced_builtins_outside_plugin_callbacks() {
    // A scheduler that computed wakeups in JS would call these replacements
    // before starting the CPU clock, so their busy time would go uncharged.
    let mut host = Host::new(
        "let hits=0;const busy=()=>{hits++;const end=Date.now()+200;while(Date.now()<end){}};\
         const replace=(target,key)=>{const original=target[key];target[key]=function(...args){busy();return original.apply(this,args)}};\
         replace(Map.prototype,'values');replace(Math,'min');replace(Math,'max');\
         Object.defineProperty(Map.prototype,'size',{get(){busy();return 0}});\
         __codemuxRegister({}, ({send}) => m => {if(m.method==='activate')setInterval(()=>send('log',{message:String(hits)}),250)});",
    );
    host.receive();
    host.send("activate", json!({}));
    let frames = host.collect(Duration::from_millis(2500));
    let logs = logs(&frames);
    assert!(host.alive());
    assert!(logs.len() >= 6, "interval stopped firing: {}", logs.len());
    assert!(logs.iter().all(|log| log["message"] == "0"));
}
#[test]
fn timer_cap_and_minimum_interval_hold_after_tampering() {
    let mut host = Host::new(
        "__codemuxRegister({}, ({send}) => m => {if(m.method!=='activate')return;\
         Math.max=(a,b)=>b;Math.min=(a,b)=>a;globalThis.Number=()=>1;Object.defineProperty(Map.prototype,'size',{get(){return 0}});\
         const first=setTimeout(()=>send('log',{message:'cleared timer fired'}),100);\
         for(let i=0;i<124;i++)setTimeout(()=>{},60000);\
         let fires=0;setInterval(()=>{fires++},1);\
         setTimeout((a,b)=>send('log',{message:a+b}),100,'arguments ','passed');\
         setTimeout(()=>send('log',{message:JSON.stringify({fires})}),1000);\
         let limited=false;try{setTimeout(()=>{},1)}catch(e){limited=e.message==='Timer limit'}\
         clearTimeout(first);let reused=true;try{setTimeout(()=>{},60000)}catch{reused=false}\
         send('log',{message:JSON.stringify({limited,reused})});});",
    );
    host.receive();
    host.send("activate", json!({}));
    let frames = host.collect(Duration::from_millis(1600));
    let logs: Vec<_> = logs(&frames)
        .into_iter()
        .map(|log| log["message"].as_str().unwrap().to_owned())
        .collect();
    assert!(host.alive());
    assert_eq!(logs[0], r#"{"limited":true,"reused":true}"#);
    assert!(logs.contains(&"arguments passed".to_owned()));
    assert!(!logs.contains(&"cleared timer fired".to_owned()));
    let report: Value = serde_json::from_str(logs.last().unwrap()).unwrap();
    let fires = report["fires"].as_u64().unwrap();
    assert!(
        (5..=11).contains(&fires),
        "a 1 ms interval fired {fires} times in 1 s"
    );
    // Timer ticks are not parent calls and send no yield frames.
    assert_eq!(frames.iter().filter(|f| f["method"] == "ready").count(), 1);
}
#[test]
fn rolling_cpu_window_stops_repeated_busy_callbacks() {
    let mut host = Host::new(&format!(
        "__codemuxRegister({{}}, () => m => {{if(m.method==='command.execute'){}}});",
        spin(220)
    ));
    host.receive();
    host.call(1, "activate", json!({}));
    assert!(host.yielded(1));
    // Each 220 ms call fits the 250 ms callback budget; the fifth exceeds
    // 1 s of JS in the rolling 5 s window.
    for id in 2..=5 {
        host.call(id, "command.execute", json!({"id":"hello"}));
        assert!(host.yielded(id), "call {id} was stopped early");
    }
    host.call(6, "command.execute", json!({"id":"hello"}));
    assert!(!host.yielded(6));
    host.stopped();
}
#[test]
fn ordinary_callbacks_have_a_shorter_budget_than_activation() {
    let mut host = Host::new(&format!(
        "__codemuxRegister({{}}, () => m => {{if(m.method==='activate'||m.method==='command.execute'){}}});",
        spin(300)
    ));
    host.receive();
    host.call(1, "activate", json!({}));
    assert!(host.yielded(1), "300 ms fits the activation budget");
    let started = Instant::now();
    host.call(2, "command.execute", json!({"id":"hello"}));
    assert!(!host.yielded(2));
    host.stopped();
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(240), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
}
#[test]
fn logs_are_rate_limited_sanitized_and_truncated() {
    let mut host = Host::new(
        "__codemuxRegister({}, ({send}) => m => {if(m.method!=='activate'&&m.method!=='command.execute')return;\
         for(let i=0;i<3;i++)console.warn('w'.repeat(5000));\
         for(let i=0;i<5;i++)send('log',{message:'\\u0007'+'r'.repeat(5000),level:'private'});});",
    );
    host.receive();
    host.call(1, "activate", json!({}));
    let frames = host.collect(Duration::from_millis(300));
    let entries = logs(&frames);
    assert_eq!(entries.len(), 5, "five entries per second");
    for (index, log) in entries.iter().enumerate() {
        let message = log["message"].as_str().unwrap();
        assert_eq!(message.chars().count(), 1024);
        assert!(!message.contains('\u{7}'));
        assert_eq!(log["level"], if index < 3 { "warn" } else { "log" });
    }
    std::thread::sleep(Duration::from_millis(800));
    host.call(2, "command.execute", json!({"id":"hello"}));
    assert_eq!(logs(&host.collect(Duration::from_millis(300))).len(), 5);
    assert!(host.alive(), "excess logs are dropped, not fatal");
}
#[test]
fn request_quota_answers_excess_then_stops_repeated_violations() {
    let mut host = Host::new(
        "let seq=0;__codemuxRegister({}, ({send}) => m => {\
         if(!m.method){if(m.error)send('log',{message:m.id+':'+m.error.data.code+':'+m.error.message});return}\
         if(m.method==='command.execute')for(let i=0;i<m.params.count;i++)send('host.request',{operation:'settings.get',params:{}},++seq);});",
    );
    host.receive();
    host.call(1, "activate", json!({}));
    assert!(host.yielded(1));
    host.call(2, "command.execute", json!({"id":"hello","count":21}));
    let frames = host.collect(Duration::from_millis(300));
    let requests = frames
        .iter()
        .filter(|f| f["method"] == "host.request")
        .count();
    assert_eq!(requests, 20, "a burst of 20 reaches the parent");
    assert_eq!(
        logs(&frames)[0]["message"],
        "21:RESOURCE_LIMIT:Plugin request quota exceeded"
    );
    assert!(host.alive(), "one excess request is answered, not fatal");
    // Five violations within 10 s stop the generation.
    host.call(3, "command.execute", json!({"id":"hello","count":4}));
    let frames = host.collect(Duration::from_secs(2));
    assert!(!frames.iter().any(|f| f["method"] == "host.request"));
    host.stopped();
}
#[test]
fn malformed_child_frames_stop_only_when_repeated() {
    let mut host = Host::new(
        "__codemuxRegister({}, ({send}) => m => {if(m.method!=='command.execute')return;\
         for(let i=0;i<m.params.count;i++)send('undeclared.method',{});send('log',{message:'alive'});});",
    );
    host.receive();
    host.call(1, "activate", json!({}));
    assert!(host.yielded(1));
    host.call(2, "command.execute", json!({"id":"hello","count":1}));
    assert!(host.yielded(2));
    host.call(3, "command.execute", json!({"id":"hello","count":0}));
    assert!(host.yielded(3), "one malformed frame is dropped, not fatal");
    host.call(4, "command.execute", json!({"id":"hello","count":4}));
    let frames = host.collect(Duration::from_secs(2));
    assert!(!logs(&frames).iter().any(|log| log["message"] == "alive"));
    host.stopped();
}
#[test]
fn ui_batches_over_thirty_per_second_wait_instead_of_faulting() {
    let mut host = Host::new(
        "__codemuxRegister({}, ({send}) => m => {if(m.method==='command.execute')for(let i=0;i<m.params.count;i++)send('ui.patch',{viewId:'view',records:[]});});",
    );
    host.receive();
    host.call(1, "activate", json!({}));
    assert!(host.yielded(1));
    let started = Instant::now();
    host.call(2, "command.execute", json!({"id":"hello","count":38}));
    let mut arrivals = Vec::new();
    while arrivals.len() < 38 {
        if host.receive()["method"] == "ui.patch" {
            arrivals.push(started.elapsed());
        }
    }
    assert!(arrivals[29] < Duration::from_millis(500));
    // Released after the 1 s window plus a margin for arrival jitter.
    assert!(
        arrivals[30] >= Duration::from_millis(1040),
        "{:?}",
        arrivals[30]
    );
    assert!(host.alive());
    // Waiting batches are bounded: a ninth one faults the generation.
    host.call(3, "command.execute", json!({"id":"hello","count":39}));
    host.collect(Duration::from_secs(2));
    host.stopped();
}
#[test]
fn oversized_requests_are_answered_and_oversized_batches_stop_the_host() {
    let mut host = Host::new(
        "__codemuxRegister({}, ({send}) => m => {\
         if(!m.method){if(m.error)send('log',{message:m.id+':'+m.error.data.code+':'+m.error.message});return}\
         if(m.method!=='command.execute')return;const text='x'.repeat(1100000);\
         if(m.params.request)send('host.request',{operation:'settings.get',params:{text}},7);\
         else send('ui.patch',{viewId:'view',records:[[2,'b',text]]});});",
    );
    host.receive();
    host.call(1, "activate", json!({}));
    assert!(host.yielded(1));
    host.call(2, "command.execute", json!({"id":"hello","request":true}));
    let frames = host.collect(Duration::from_millis(500));
    assert!(!frames.iter().any(|f| f["method"] == "host.request"));
    assert_eq!(
        logs(&frames)[0]["message"],
        "7:RESOURCE_LIMIT:Host request exceeds 1 MiB"
    );
    assert!(host.alive(), "an oversized request is answered, not fatal");
    // A dropped batch would leave the renderer's tree silently stale.
    host.call(3, "command.execute", json!({"id":"hello","request":false}));
    assert_eq!(
        host.reason().trim(),
        "Plugin host stopped: Outgoing frame limit"
    );
}
#[cfg(target_os = "linux")]
#[test]
fn inherited_descriptors_are_closed_before_plugin_code_runs() {
    // The shell leaves descriptor 9 open across exec, as a launcher might.
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "exec 9</dev/null; exec \"$0\"",
        env!("CARGO_BIN_EXE_codemux-addon-host"),
    ]);
    let mut host = Host::with(command, "__codemuxRegister({},()=>()=>{});");
    host.receive();
    host.call(1, "activate", json!({}));
    assert!(host.yielded(1));
    let mut descriptors: Vec<u32> = std::fs::read_dir(format!("/proc/{}/fd", host.child.id()))
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    descriptors.sort_unstable();
    assert_eq!(descriptors, [0, 1, 2]);
}
