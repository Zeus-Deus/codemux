//! The stdio transport for Codemux's MCP servers: newline-delimited JSON-RPC
//! read from one stream and written to another.
//!
//! Cheap methods are answered in arrival order before the next line is read.
//! Other requests run concurrently and can be cancelled with
//! `notifications/cancelled`; a cancelled request gets no reply, as MCP asks.
//! Nothing is ever written that the client did not ask for, so a client that
//! sends one request and reads exactly one line keeps working.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinError, JoinSet};

use super::{parse, Incoming, Notification, Request, RequestId, Response, RpcError};

/// How a [`Handler`] answers one request.
pub enum Reply {
    /// Written before the next line is read, so replies keep arrival order.
    /// For cheap methods that set up or describe the session: `initialize`,
    /// `ping`, `tools/list`.
    Now(Result<Value, RpcError>),
    /// Runs alongside later requests and can be cancelled. `parks` marks work
    /// that waits on events, such as a long `thread_wait`: when the input
    /// closes nobody can read its result, so it is cancelled instead of
    /// awaited.
    Later {
        future: BoxFuture<'static, Result<Value, RpcError>>,
        parks: bool,
    },
    /// Like `Later`, for work that waits its turn in a queue the handler
    /// keeps, such as tools that must run one at a time in arrival order. It
    /// does not count toward [`MAX_IN_FLIGHT`]: a burst of such calls is
    /// queued, not refused, as a sequential server would have done. It never
    /// parks.
    Queued(BoxFuture<'static, Result<Value, RpcError>>),
}

impl Reply {
    /// Rewrite the successful result whenever it arrives; errors pass
    /// through. Scheduling (`Now`, `Later`, `Queued`, `parks`) is unchanged.
    pub fn map_result(self, rewrite: impl FnOnce(Value) -> Value + Send + 'static) -> Self {
        match self {
            Self::Now(outcome) => Self::Now(outcome.map(rewrite)),
            Self::Later { future, parks } => Self::Later { future: future.map(|outcome| outcome.map(rewrite)).boxed(), parks },
            Self::Queued(future) => Self::Queued(future.map(|outcome| outcome.map(rewrite)).boxed()),
        }
    }
}

pub trait Handler: Send + Sync + 'static {
    /// Called on the reader, one request at a time and in arrival order, so
    /// work a handler queues here keeps that order.
    fn request(&self, request: Request) -> Reply;
}

/// Most `Later` requests running at once. Past it a request is refused
/// rather than queued, so the reader stays free to read cancellations.
/// `Queued` requests are not counted.
pub const MAX_IN_FLIGHT: usize = 64;

/// Serve MCP over `input` and `output` until `input` ends.
///
/// At end of input, parked requests are cancelled and every other running
/// request finishes and is answered, so piped one-shot use
/// (`printf '...' | codemux mcp`) gets its replies. Fails when reading the
/// input or writing the output fails.
pub async fn serve<I, O, H>(input: I, output: O, handler: Arc<H>) -> Result<(), String>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin + Send + 'static,
    H: Handler,
{
    let (out, lines) = mpsc::unbounded_channel::<String>();
    let mut writer = tokio::spawn(write_lines(output, lines));
    let in_flight = InFlight::default();
    let mut tasks = JoinSet::new();
    let mut input = BufReader::new(input);
    let mut line = Vec::new();
    let read = loop {
        line.clear();
        let read = tokio::select! {
            read = input.read_until(b'\n', &mut line) => read,
            // The writer only stops on its own when the output fails.
            written = &mut writer => {
                tasks.abort_all();
                return Err(writer_result(written).err().unwrap_or_else(|| "stdout closed".into()));
            }
        };
        match read {
            Ok(0) => break Ok(()),
            Ok(_) => handle_line(&line, handler.as_ref(), &in_flight, &mut tasks, &out),
            Err(error) => break Err(format!("stdin read error: {error}")),
        }
        while tasks.try_join_next().is_some() {}
    };
    in_flight.cancel_parked();
    while tasks.join_next().await.is_some() {}
    drop(out);
    let written = writer_result(writer.await);
    read.and(written)
}

fn handle_line<H: Handler>(
    line: &[u8],
    handler: &H,
    in_flight: &InFlight,
    tasks: &mut JoinSet<()>,
    out: &mpsc::UnboundedSender<String>,
) {
    let line = line.trim_ascii();
    if line.is_empty() {
        return;
    }
    let message: Value = match serde_json::from_slice(line) {
        Ok(message) => message,
        Err(error) => return send(out, Response::error(None, RpcError::parse_error(error))),
    };
    // An invalid message whose id is still readable is answered with that id,
    // so a client matching replies by id is not left waiting; otherwise null.
    let error_id = message.get("id").and_then(RequestId::from_value);
    let request = match parse(message) {
        Ok(Incoming::Request(request)) => request,
        Ok(Incoming::Notification(Notification::Cancelled { request_id, .. })) => {
            in_flight.cancel(&request_id);
            return;
        }
        Ok(Incoming::Notification(_) | Incoming::Response) => return,
        Err(error) => return send(out, Response::error(error_id, error)),
    };
    let id = request.id.clone();
    // JSON-RPC ids must be unique among a client's open requests.
    if in_flight.contains(&id) {
        return send(out, Response::error(Some(id), RpcError::invalid_request("Request id is already in flight")));
    }
    let (future, parks, counted) = match handler.request(request) {
        Reply::Now(outcome) => return send(out, Response::from_outcome(id, outcome)),
        Reply::Later { future, parks } => (future, parks, true),
        Reply::Queued(future) => (future, false, false),
    };
    // Dropping `future` here abandons the work before it starts.
    let Some(mut call) = in_flight.begin(id.clone(), parks, counted) else {
        return send(out, Response::error(Some(id), RpcError::internal("Too many concurrent requests")));
    };
    let out = out.clone();
    tasks.spawn(async move {
        let outcome = tokio::select! {
            biased;
            () = call.cancelled() => None,
            outcome = AssertUnwindSafe(future).catch_unwind() => Some(outcome.unwrap_or_else(|_| {
                Err(RpcError::internal("The request failed unexpectedly"))
            })),
        };
        // Free the id before replying: the client may reuse it as soon as it
        // reads the reply.
        drop(call);
        if let Some(outcome) = outcome {
            send(&out, Response::from_outcome(id, outcome));
        }
    });
}

fn send(out: &mpsc::UnboundedSender<String>, response: Response) {
    // Fails only once the writer has stopped, which `serve` reports itself.
    let _ = out.send(response.to_line());
}

async fn write_lines<O: AsyncWrite + Unpin>(
    mut output: O,
    mut lines: mpsc::UnboundedReceiver<String>,
) -> Result<(), String> {
    while let Some(mut line) = lines.recv().await {
        line.push('\n');
        output.write_all(line.as_bytes()).await.map_err(|error| format!("stdout write error: {error}"))?;
        output.flush().await.map_err(|error| format!("stdout write error: {error}"))?;
    }
    Ok(())
}

fn writer_result(joined: Result<Result<(), String>, JoinError>) -> Result<(), String> {
    joined.unwrap_or_else(|error| Err(format!("stdout writer failed: {error}")))
}

/// Requests running on one connection, keyed by their JSON-RPC id.
#[derive(Default)]
struct InFlight {
    calls: Arc<Mutex<HashMap<RequestId, Entry>>>,
}

struct Entry {
    cancel: watch::Sender<bool>,
    parks: bool,
    /// Counts toward [`MAX_IN_FLIGHT`] (`Reply::Later`, not `Reply::Queued`).
    counted: bool,
}

impl InFlight {
    fn lock(&self) -> MutexGuard<'_, HashMap<RequestId, Entry>> {
        lock(&self.calls)
    }

    fn contains(&self, id: &RequestId) -> bool {
        self.lock().contains_key(id)
    }

    /// `None` for a `counted` request when [`MAX_IN_FLIGHT`] counted requests
    /// are already running. The caller has checked that `id` is not one of
    /// them.
    fn begin(&self, id: RequestId, parks: bool, counted: bool) -> Option<Call> {
        let mut calls = self.lock();
        if counted && calls.values().filter(|entry| entry.counted).count() >= MAX_IN_FLIGHT {
            return None;
        }
        let (cancel, cancelled) = watch::channel(false);
        calls.insert(id.clone(), Entry { cancel, parks, counted });
        Some(Call { id, cancelled, calls: Arc::clone(&self.calls) })
    }

    /// An unknown or finished id is ignored, as MCP asks.
    fn cancel(&self, id: &RequestId) {
        if let Some(entry) = self.lock().get(id) {
            entry.cancel.send_replace(true);
        }
    }

    fn cancel_parked(&self) {
        for entry in self.lock().values().filter(|entry| entry.parks) {
            entry.cancel.send_replace(true);
        }
    }
}

/// One running request. Dropping it frees the id.
struct Call {
    id: RequestId,
    cancelled: watch::Receiver<bool>,
    calls: Arc<Mutex<HashMap<RequestId, Entry>>>,
}

impl Call {
    async fn cancelled(&mut self) {
        // The sender lives in the map until this call drops, so the wait can
        // only end with a cancellation.
        if self.cancelled.wait_for(|cancelled| *cancelled).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        lock(&self.calls).remove(&self.id);
    }
}

fn lock(calls: &Mutex<HashMap<RequestId, Entry>>) -> MutexGuard<'_, HashMap<RequestId, Entry>> {
    calls.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, DuplexStream, Lines};
    use tokio::sync::Notify;
    use tokio::task::JoinHandle;

    /// A handler whose methods say how they answer:
    /// - `echo`: `Now`, returns its params;
    /// - `slow`: `Later`, finishes when the test releases `gate`;
    /// - `park`: `Later` with `parks`, never finishes on its own;
    /// - `quick`: `Later`, finishes at once;
    /// - `queue`: `Queued`, finishes when the test releases `gate`;
    /// - `boom`: `Later`, panics.
    #[derive(Default)]
    struct Fake {
        gate: Arc<Notify>,
        /// Labels of `slow` and `park` work dropped before it finished.
        dropped: Arc<Mutex<Vec<String>>>,
    }

    /// Records its label in `dropped` unless the work finished.
    struct Unfinished {
        label: String,
        finished: bool,
        dropped: Arc<Mutex<Vec<String>>>,
    }

    impl Drop for Unfinished {
        fn drop(&mut self) {
            if !self.finished {
                self.dropped.lock().unwrap().push(self.label.clone());
            }
        }
    }

    impl Handler for Fake {
        fn request(&self, request: Request) -> Reply {
            let label = request.params.get("label").and_then(Value::as_str).unwrap_or_default().to_owned();
            let mut unfinished =
                Unfinished { label: label.clone(), finished: false, dropped: Arc::clone(&self.dropped) };
            match request.method.as_str() {
                method @ ("slow" | "queue") => {
                    let gate = Arc::clone(&self.gate);
                    let future: BoxFuture<'static, Result<Value, RpcError>> = Box::pin(async move {
                        // Bind the whole guard: naming only its field would
                        // capture a copy of the flag and drop the guard now.
                        let mut unfinished = unfinished;
                        gate.notified().await;
                        unfinished.finished = true;
                        Ok(json!({ "done": label }))
                    });
                    if method == "queue" {
                        Reply::Queued(future)
                    } else {
                        Reply::Later { future, parks: false }
                    }
                }
                "park" => Reply::Later {
                    future: Box::pin(async move {
                        let _unfinished = unfinished;
                        std::future::pending::<Result<Value, RpcError>>().await
                    }),
                    parks: true,
                },
                method => {
                    unfinished.finished = true;
                    match method {
                        "echo" => Reply::Now(Ok(request.params)),
                        "quick" => Reply::Later { future: Box::pin(async { Ok(json!("quick")) }), parks: false },
                        "boom" => Reply::Later { future: Box::pin(async { panic!("handler bug") }), parks: false },
                        other => Reply::Now(Err(RpcError::method_not_found(other))),
                    }
                }
            }
        }
    }

    struct Session {
        input: Option<DuplexStream>,
        output: Lines<BufReader<DuplexStream>>,
        served: JoinHandle<Result<(), String>>,
    }

    impl Session {
        fn start(handler: Arc<Fake>) -> Self {
            let (input, server_input) = tokio::io::duplex(1 << 16);
            let (server_output, output) = tokio::io::duplex(1 << 16);
            let served = tokio::spawn(serve(server_input, server_output, handler));
            Self { input: Some(input), output: BufReader::new(output).lines(), served }
        }

        async fn send(&mut self, message: Value) {
            self.send_raw(&message.to_string()).await;
        }

        async fn send_raw(&mut self, line: &str) {
            let input = self.input.as_mut().expect("input still open");
            input.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        }

        async fn next(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(5), self.output.next_line())
                .await
                .expect("a reply within 5 s")
                .unwrap()
                .expect("an open output");
            serde_json::from_str(&line).unwrap()
        }

        /// Nothing more is written within a short grace period.
        async fn quiet(&mut self) {
            let next = tokio::time::timeout(Duration::from_millis(200), self.output.next_line()).await;
            assert!(next.is_err(), "unexpected output: {next:?}");
        }

        fn close_input(&mut self) {
            self.input.take();
        }

        /// Closes the input and collects every remaining line until the server ends.
        async fn finish(mut self) -> Vec<Value> {
            self.close_input();
            let mut lines = Vec::new();
            while let Some(line) =
                tokio::time::timeout(Duration::from_secs(5), self.output.next_line()).await.expect("output ends").unwrap()
            {
                lines.push(serde_json::from_str(&line).unwrap());
            }
            tokio::time::timeout(Duration::from_secs(5), self.served).await.expect("serve ends").unwrap().unwrap();
            lines
        }
    }

    fn request(id: Value, method: &str, label: &str) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": { "label": label } })
    }

    fn cancel(id: Value) -> Value {
        json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": id } })
    }

    #[tokio::test]
    async fn stdio_answers_ping_while_a_later_call_is_running() {
        let handler = Arc::new(Fake::default());
        let mut session = Session::start(Arc::clone(&handler));
        session.send(request(json!(1), "slow", "wait")).await;
        session.send(request(json!(2), "echo", "ping")).await;
        let reply = session.next().await;
        assert_eq!(reply["id"], 2, "the cheap request must not wait behind the running one");
        assert_eq!(reply["result"]["label"], "ping");
        handler.gate.notify_one();
        let reply = session.next().await;
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["done"], "wait");
    }

    #[tokio::test]
    async fn stdio_cancel_ends_a_running_call_without_a_reply() {
        let handler = Arc::new(Fake::default());
        let mut session = Session::start(Arc::clone(&handler));
        session.send(request(json!("w"), "park", "parked")).await;
        session.send(cancel(json!("w"))).await;
        // A later request with the same id is accepted once the cancelled one is gone.
        session.send(request(json!(9), "echo", "after")).await;
        assert_eq!(session.next().await["id"], 9);
        session.quiet().await;
        for _ in 0..50 {
            if !handler.dropped.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*handler.dropped.lock().unwrap(), ["parked"], "the cancelled work must stop");
        session.send(request(json!("w"), "quick", "reused")).await;
        let reply = session.next().await;
        assert_eq!((reply["id"].clone(), reply["result"].clone()), (json!("w"), json!("quick")));
    }

    #[tokio::test]
    async fn stdio_rejects_a_duplicate_in_flight_id() {
        let handler = Arc::new(Fake::default());
        let mut session = Session::start(Arc::clone(&handler));
        session.send(request(json!(1), "slow", "first")).await;
        session.send(request(json!(1), "echo", "second")).await;
        let reply = session.next().await;
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["error"]["code"], -32600);
        assert!(reply["error"]["message"].as_str().unwrap().contains("already in flight"));
        // The string id "1" is a different id.
        session.send(request(json!("1"), "echo", "string")).await;
        assert_eq!(session.next().await["id"], "1");
        handler.gate.notify_one();
        assert_eq!(session.next().await["result"]["done"], "first");
        // Once answered, the id can be used again straight away.
        session.send(request(json!(1), "echo", "again")).await;
        assert_eq!(session.next().await["result"]["label"], "again");
    }

    #[tokio::test]
    async fn stdio_drains_in_flight_calls_after_stdin_closes() {
        let handler = Arc::new(Fake::default());
        let mut session = Session::start(Arc::clone(&handler));
        session.send(request(json!(1), "slow", "drained")).await;
        session.send(request(json!(2), "quick", "quick")).await;
        assert_eq!(session.next().await["id"], 2);
        session.close_input();
        // The running call is still answered after the input closed.
        tokio::time::sleep(Duration::from_millis(50)).await;
        handler.gate.notify_one();
        let lines = session.finish().await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[0]["result"]["done"], "drained");
    }

    #[tokio::test]
    async fn stdio_cancels_parked_calls_when_stdin_closes() {
        let handler = Arc::new(Fake::default());
        let mut session = Session::start(Arc::clone(&handler));
        session.send(request(json!(1), "park", "parked")).await;
        session.send(request(json!(2), "slow", "drained")).await;
        session.close_input();
        tokio::time::sleep(Duration::from_millis(50)).await;
        handler.gate.notify_one();
        let lines = session.finish().await;
        assert_eq!(lines.len(), 1, "a parked call gets no reply: {lines:?}");
        assert_eq!(lines[0]["id"], 2);
        assert_eq!(*handler.dropped.lock().unwrap(), ["parked"]);
    }

    #[tokio::test]
    async fn stdio_reports_parse_errors_with_null_id_and_keeps_reading() {
        let mut session = Session::start(Arc::new(Fake::default()));
        session.send_raw("{not json").await;
        let reply = session.next().await;
        assert_eq!(reply["id"], Value::Null);
        assert_eq!(reply["error"]["code"], -32700);
        session.send(json!([request(json!(1), "echo", "batched")])).await;
        let reply = session.next().await;
        assert_eq!((reply["id"].clone(), reply["error"]["code"].clone()), (Value::Null, json!(-32600)));
        session.send(json!({ "jsonrpc": "2.0", "id": null, "method": "echo" })).await;
        assert_eq!(session.next().await["error"]["code"], -32600);
        session.send_raw("   ").await;
        session.send(request(json!(3), "echo", "still reading")).await;
        assert_eq!(session.next().await["result"]["label"], "still reading");
    }

    #[tokio::test]
    async fn stdio_answers_envelope_errors_with_a_readable_id() {
        let mut session = Session::start(Arc::new(Fake::default()));
        for (message, id) in [
            (json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": [1] }), json!(5)),
            (json!({ "jsonrpc": "2.0", "id": "s" }), json!("s")),
            (json!({ "jsonrpc": "2.0", "id": 6, "method": 7 }), json!(6)),
            // An id MCP does not allow cannot be echoed.
            (json!({ "jsonrpc": "2.0", "id": 1.5, "method": "echo" }), Value::Null),
        ] {
            session.send(message.clone()).await;
            let reply = session.next().await;
            assert_eq!((reply["id"].clone(), reply["error"]["code"].clone()), (id, json!(-32600)), "{message}");
        }
    }

    #[tokio::test]
    async fn stdio_rejects_invalid_utf8_lines_as_parse_errors() {
        let mut session = Session::start(Arc::new(Fake::default()));
        let input = session.input.as_mut().unwrap();
        input.write_all(b"\xff\xfe{}\n").await.unwrap();
        let reply = session.next().await;
        assert_eq!(reply["error"]["code"], -32700);
        session.send(request(json!(1), "echo", "next")).await;
        assert_eq!(session.next().await["id"], 1);
    }

    #[tokio::test]
    async fn stdio_refuses_calls_beyond_max_in_flight() {
        let handler = Arc::new(Fake::default());
        let mut session = Session::start(Arc::clone(&handler));
        for id in 0..MAX_IN_FLIGHT {
            session.send(request(json!(id), "park", "held")).await;
        }
        session.send(request(json!("over"), "park", "refused")).await;
        let reply = session.next().await;
        assert_eq!(reply["id"], "over");
        assert_eq!(reply["error"]["code"], -32603);
        // Cheap requests still work, and a slot frees up after a cancel.
        session.send(request(json!("ping"), "echo", "ping")).await;
        assert_eq!(session.next().await["id"], "ping");
        session.send(cancel(json!(0))).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        session.send(request(json!("fits"), "quick", "fits")).await;
        assert_eq!(session.next().await["id"], "fits");
        assert!(handler.dropped.lock().unwrap().contains(&"refused".to_owned()), "refused work is dropped unstarted");
    }

    #[tokio::test]
    async fn stdio_queues_calls_past_max_in_flight_without_refusing_them() {
        let handler = Arc::new(Fake::default());
        let mut session = Session::start(Arc::clone(&handler));
        for id in 0..MAX_IN_FLIGHT {
            session.send(request(json!(id), "park", "held")).await;
        }
        // Queued work waits its turn instead of failing, however much is
        // already running or queued.
        for id in 0..2 * MAX_IN_FLIGHT {
            session.send(request(json!(format!("q{id}")), "queue", "queued")).await;
        }
        session.send(request(json!("ping"), "echo", "ping")).await;
        assert_eq!(session.next().await["id"], "ping", "no queued call was refused");
        // Running calls are still capped.
        session.send(request(json!("over"), "park", "refused")).await;
        assert_eq!(session.next().await["error"]["code"], -32603);
        handler.gate.notify_one();
        let reply = session.next().await;
        assert!(reply["id"].as_str().is_some_and(|id| id.starts_with('q')), "{reply}");
        assert_eq!(reply["result"]["done"], "queued");
    }

    #[tokio::test]
    async fn stdio_never_writes_for_notifications() {
        let mut session = Session::start(Arc::new(Fake::default()));
        session.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
        session.send(json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": { "progressToken": 1 } })).await;
        session.send(cancel(json!("never-started"))).await;
        session.send(json!({ "jsonrpc": "2.0", "method": "echo" })).await;
        session.send(json!({ "jsonrpc": "2.0", "id": 5, "result": {} })).await;
        session.send(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -1, "message": "x" } })).await;
        session.quiet().await;
        assert!(session.finish().await.is_empty());
    }

    #[tokio::test]
    async fn stdio_now_replies_keep_arrival_order() {
        let mut session = Session::start(Arc::new(Fake::default()));
        for id in 0..20 {
            session.send(request(json!(id), "echo", "ordered")).await;
        }
        for id in 0..20 {
            assert_eq!(session.next().await["id"], id);
        }
    }

    #[tokio::test]
    async fn stdio_answers_a_panicking_call_with_an_internal_error() {
        let mut session = Session::start(Arc::new(Fake::default()));
        session.send(request(json!(1), "boom", "")).await;
        let reply = session.next().await;
        assert_eq!((reply["id"].clone(), reply["error"]["code"].clone()), (json!(1), json!(-32603)));
        session.send(request(json!(2), "echo", "alive")).await;
        assert_eq!(session.next().await["id"], 2);
    }

    #[tokio::test]
    async fn stdio_reports_an_output_failure() {
        let (mut input, server_input) = tokio::io::duplex(1024);
        let (server_output, output) = tokio::io::duplex(1024);
        drop(output);
        let served = tokio::spawn(serve(server_input, server_output, Arc::new(Fake::default())));
        input.write_all(format!("{}\n", request(json!(1), "echo", "lost")).as_bytes()).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), served).await.expect("serve ends").unwrap();
        assert!(result.unwrap_err().contains("stdout"), "the write failure is reported");
    }

    #[tokio::test]
    async fn map_result_rewrites_results_and_keeps_errors_and_scheduling() {
        let mark = |value: Value| json!({ "marked": value });
        let Reply::Now(Ok(now)) = Reply::Now(Ok(json!(1))).map_result(mark) else { panic!("still Now") };
        assert_eq!(now, json!({ "marked": 1 }));
        let Reply::Now(Err(error)) = Reply::Now(Err(RpcError::internal("no"))).map_result(mark) else { panic!("still Now") };
        assert_eq!(error.code, -32603, "errors pass through");
        let later = Reply::Later { future: async { Ok(json!(2)) }.boxed(), parks: true }.map_result(mark);
        let Reply::Later { future, parks: true } = later else { panic!("still a parking Later") };
        assert_eq!(future.await.unwrap(), json!({ "marked": 2 }));
        let Reply::Queued(future) = Reply::Queued(async { Ok(json!(3)) }.boxed()).map_result(mark) else { panic!("still Queued") };
        assert_eq!(future.await.unwrap(), json!({ "marked": 3 }));
    }
}
