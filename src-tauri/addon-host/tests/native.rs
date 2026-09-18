use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_codemux-addon-host"))
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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
        writeln!(self.input.as_mut().unwrap(),"{}",json!({"jsonrpc":"2.0","generation":"native-test","id":1,"method":method,"params":params})).unwrap();
    }
    fn receive(&self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(2))
            .expect("host reply deadline")
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
