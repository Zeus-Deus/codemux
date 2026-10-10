//! Integration tests for [`codemux_lib::json_rpc_child::JsonRpcChild`].
//!
//! Every test spawns the `fake_rpc_child` helper binary and exercises a
//! specific facet of the helper: basic roundtrip, notifications both ways,
//! server-initiated requests, timeouts, child-exit cleanup, and graceful
//! shutdown. The helper binary lives under
//! `tests/helpers/fake_rpc_child/main.rs` and is wired up as a `[[bin]]`
//! target in `Cargo.toml`.

use std::path::PathBuf;
use std::time::Duration;

use codemux_lib::json_rpc_child::{JsonRpcChild, RpcChildError, SpawnConfig};
use serde_json::{json, Value};

#[cfg(target_os = "linux")]
struct OwnedPeer(i32);
#[cfg(target_os = "linux")]
impl Drop for OwnedPeer {
    fn drop(&mut self) {
        // Failure-safe cleanup of this fixture's exact subprocess only.
        if PathBuf::from(format!("/proc/{}", self.0)).exists() {
            unsafe { libc::kill(self.0, libc::SIGKILL); }
        }
    }
}

#[cfg(target_os = "linux")]
async fn nonreading_peer() -> (std::sync::Arc<JsonRpcChild>, OwnedPeer, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let child = std::sync::Arc::new(JsonRpcChild::spawn(SpawnConfig {
        program: "/usr/bin/python3".into(),
        args: vec!["-I".into(), "-u".into(), "-c".into(),
            "import json,os,time; print(json.dumps({'jsonrpc':'2.0','method':'ready','params':{'pid':os.getpid()}}),flush=True); time.sleep(60)".into()],
        env: std::collections::HashMap::from([
            ("HOME".into(), dir.path().to_string_lossy().into_owned()),
            ("TMPDIR".into(), dir.path().to_string_lossy().into_owned()),
        ]),
        cwd: Some(dir.path().to_owned()),
        ..config()
    }).await.unwrap());
    let mut notes = child.notifications();
    let ready = tokio::time::timeout(Duration::from_secs(2), notes.recv()).await.unwrap().unwrap();
    let peer = OwnedPeer(ready.params["pid"].as_i64().unwrap() as i32);
    (child, peer, dir)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn transport_shutdown_reaps_with_nonreading_stdin() {
    let (child, peer, _dir) = nonreading_peer().await;
    let write = child.notify("fill", json!({"text":"x".repeat(4 * 1024 * 1024)}));
    tokio::pin!(write);
    assert!(futures_util::poll!(&mut write).is_pending(), "large actual stdin write must block");
    let outcome = tokio::time::timeout(Duration::from_secs(4), child.shutdown()).await;
    let reaped = !PathBuf::from(format!("/proc/{}", peer.0)).exists();
    drop(write);
    drop(peer);
    assert!(matches!(outcome, Ok(Ok(()))), "shutdown must reap despite a held writer: {outcome:?}");
    assert!(reaped, "successful shutdown must have actually reaped the peer");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn transport_concurrent_shutdown_joins_owned_reap() {
    let (child, peer, _dir) = nonreading_peer().await;
    let write = child.notify("fill", json!({"text":"x".repeat(4 * 1024 * 1024)}));
    tokio::pin!(write);
    assert!(futures_util::poll!(&mut write).is_pending());
    let result = tokio::time::timeout(Duration::from_secs(6), async {
        tokio::join!(child.shutdown(), child.shutdown())
    }).await;
    let reaped = !PathBuf::from(format!("/proc/{}", peer.0)).exists();
    drop(write);
    drop(peer);
    assert!(matches!(result, Ok((Ok(()), Ok(())))), "both callers must join cleanup, not wedge/time out: {result:?}");
    assert!(reaped);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn transport_cancelled_shutdown_initiator_does_not_strand_cleanup() {
    let (child, peer, _dir) = nonreading_peer().await;
    {
        let write = child.notify("fill", json!({"text":"x".repeat(4 * 1024 * 1024)}));
        tokio::pin!(write);
        assert!(futures_util::poll!(&mut write).is_pending());
        {
            let first = child.shutdown();
            tokio::pin!(first);
            assert!(futures_util::poll!(&mut first).is_pending());
        } // Cancel the actual sole initiator while the writer is still held.
    } // Cancel the blocked write and release its writer guard.
    let outcome = child.shutdown().await;
    let reaped = !PathBuf::from(format!("/proc/{}", peer.0)).exists();
    drop(peer);
    assert!(outcome.is_ok(), "another caller must join initiated cleanup after cancellation: {outcome:?}");
    assert!(reaped);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn transport_inherited_pipes_are_closed_at_owned_drain_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let release = dir.path().join("release");
    let done = dir.path().join("done");
    let script = r#"import json,os,sys,time
q=json.loads(sys.stdin.readline())
pid=os.fork()
if pid == 0:
    deadline=time.monotonic()+10
    while not os.path.exists(sys.argv[1]) and time.monotonic()<deadline: time.sleep(.005)
    try:
        print(json.dumps({'jsonrpc':'2.0','method':'late','params':{}}),flush=True)
        print('late stderr',file=sys.stderr,flush=True)
    except BrokenPipeError: pass
    open(sys.argv[2],'w').write('done')
    os._exit(0)
print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':{'pid':os.getpid(),'descendant':pid}}),flush=True)
os._exit(0)
"#;
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: "/usr/bin/python3".into(),
        args: vec!["-I".into(),"-u".into(),"-c".into(),script.into(),release.to_string_lossy().into_owned(),done.to_string_lossy().into_owned()],
        cwd: Some(dir.path().to_owned()),
        env: std::collections::HashMap::from([("HOME".into(),dir.path().to_string_lossy().into_owned()),("TMPDIR".into(),dir.path().to_string_lossy().into_owned())]),
        ..config()
    }).await.unwrap();
    let mut notes = child.notifications();
    let mut incoming = child.incoming_requests().unwrap();
    let response = child.request("fork",json!({})).await.unwrap();
    let parent = OwnedPeer(response["pid"].as_i64().unwrap() as i32);
    let descendant = OwnedPeer(response["descendant"].as_i64().unwrap() as i32);
    let result = child.shutdown().await;
    let closed = tokio::time::timeout(Duration::from_millis(150), incoming.recv()).await;
    let reaped = !PathBuf::from(format!("/proc/{}",parent.0)).exists();
    std::fs::write(&release,"release").unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(2), async {
        while !done.exists() {tokio::time::sleep(Duration::from_millis(5)).await;}
    }).await;
    let late = notes.try_recv();
    drop(descendant);
    drop(parent);
    assert!(result.is_ok(),"owned shutdown must complete: {result:?}");
    assert!(reaped);
    assert!(finished.is_ok());
    assert!(matches!(closed,Ok(None)),"shutdown must join/abort retained reader, not detach it: {closed:?}");
    assert!(late.is_err(),"post-boundary descendant bytes must never be routed: {late:?}");
}

#[cfg(target_os = "linux")]
async fn asymmetric_pipe_drain(retained_fd: u8, strict: bool) {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"import json,os,sys,time
q=json.loads(sys.stdin.readline())
pid=os.fork()
if pid == 0:
    os.close(0)
    os.close(2 if sys.argv[1]=='1' else 1)
    open('fd-closed','w').write('closed')
    deadline=time.monotonic()+30
    while not os.path.exists('release') and time.monotonic()<deadline: time.sleep(.005)
    try: os.write(int(sys.argv[1]),b'{"jsonrpc":"2.0","method":"late","params":{}}\n')
    except BrokenPipeError: pass
    open('done','w').write('done')
    os._exit(0)
while not os.path.exists('fd-closed'): time.sleep(.005)
print(json.dumps({'jsonrpc':'2.0','method':'ready','params':{'pid':os.getpid(),'descendant':pid}}),flush=True)
while not os.path.exists('exit'): time.sleep(.005)
os._exit(0)
"#;
    let config = SpawnConfig {
        program: "/usr/bin/python3".into(),
        args: vec!["-I".into(), "-u".into(), "-c".into(), script.into(), retained_fd.to_string()],
        cwd: Some(dir.path().to_owned()),
        env: std::collections::HashMap::from([("HOME".into(), dir.path().to_string_lossy().into_owned()), ("TMPDIR".into(), dir.path().to_string_lossy().into_owned())]),
        default_timeout: Duration::from_secs(30),
    };
    let child = if strict { JsonRpcChild::spawn_strict(config).await } else { JsonRpcChild::spawn(config).await }.unwrap();
    let mut notes = child.notifications();
    let mut incoming = child.incoming_requests().unwrap();
    // The actual pending request is written before the peer forks; never answered.
    let pending = child.request("unanswered", json!({}));
    tokio::pin!(pending);
    let ready = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::select! {
            ready = notes.recv() => ready.unwrap(),
            result = &mut pending => panic!("peer must leave the request pending: {result:?}"),
        }
    }).await.unwrap();
    let parent = OwnedPeer(ready.params["pid"].as_i64().unwrap() as i32);
    let descendant = OwnedPeer(ready.params["descendant"].as_i64().unwrap() as i32);
    std::fs::write(dir.path().join("exit"), "exit").unwrap();
    let shutdowns = tokio::time::timeout(Duration::from_secs(6), async {
        tokio::join!(child.shutdown(), child.shutdown(), child.shutdown())
    }).await;
    let settled = tokio::time::timeout(Duration::from_millis(150), &mut pending).await;
    let closed = tokio::time::timeout(Duration::from_millis(150), incoming.recv()).await;
    let reaped = !PathBuf::from(format!("/proc/{}", parent.0)).exists();
    std::fs::write(dir.path().join("release"), "release").unwrap();
    let done = tokio::time::timeout(Duration::from_secs(2), async {
        while !dir.path().join("done").exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await;
    let late = notes.try_recv();
    drop(descendant);
    drop(parent);
    assert!(matches!(shutdowns, Ok((Ok(()), Ok(()), Ok(())))), "asymmetric fd={retained_fd} strict={strict}: watchdog must complete without double-poll panic: {shutdowns:?}");
    assert!(reaped, "owned parent must actually be reaped");
    assert!(matches!(settled, Ok(Err(RpcChildError::RpcError(_)))), "unanswered pending request must fail at the owned drain boundary: {settled:?}");
    assert!(matches!(closed, Ok(None)), "callback channel must be closed: {closed:?}");
    assert!(done.is_ok());
    assert!(late.is_err(), "no descendant bytes may route after the drain boundary: {late:?}");
}
#[cfg(target_os = "linux")]
#[tokio::test]
async fn final_asymmetric_stdout_eof_stderr_inherited_tolerant() { asymmetric_pipe_drain(2, false).await; }
#[cfg(target_os = "linux")]
#[tokio::test]
async fn final_asymmetric_stderr_eof_stdout_inherited_tolerant() { asymmetric_pipe_drain(1, false).await; }
#[cfg(target_os = "linux")]
#[tokio::test]
async fn final_asymmetric_stdout_eof_stderr_inherited_strict() { asymmetric_pipe_drain(2, true).await; }
#[cfg(target_os = "linux")]
#[tokio::test]
async fn final_asymmetric_stderr_eof_stdout_inherited_strict() { asymmetric_pipe_drain(1, true).await; }

fn helper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_rpc_child"))
}

fn config() -> SpawnConfig {
    SpawnConfig {
        program: helper_path(),
        args: vec![],
        env: Default::default(),
        cwd: None,
        default_timeout: Duration::from_secs(5),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_and_request_echo() {
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    let result = child
        .request("echo", json!({"greeting": "hello"}))
        .await
        .expect("request echo");
    assert_eq!(result, json!({"greeting": "hello"}));
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_from_us_to_child() {
    // The helper's `notify` method emits a `tick` notification echoing
    // params, then responds with `{"notified": true}`. We observe both
    // sides to confirm the outgoing notification path is wired and the
    // request still completes normally.
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    let mut notifications = child.notifications();

    // Fire first, then expect a tick notification back and a response.
    let result = child
        .request("notify", json!({"n": 42}))
        .await
        .expect("request notify");
    assert_eq!(result, json!({"notified": true}));

    // The tick notification may arrive before or after the response,
    // but our broadcast receiver queues up to 512 items so we will catch
    // it regardless of ordering.
    let note = tokio::time::timeout(Duration::from_secs(2), notifications.recv())
        .await
        .expect("timed out waiting for tick")
        .expect("broadcast recv");
    assert_eq!(note.method, "tick");
    assert_eq!(note.params, json!({"n": 42}));

    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_from_child_to_us() {
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    let mut notifications = child.notifications();
    // Send a notification to the child (no id) that it handles as
    // `emit_notification`, which makes it emit a `heartbeat` notification.
    child
        .notify("emit_notification", json!({"kind": "beat"}))
        .await
        .expect("notify");
    let note = tokio::time::timeout(Duration::from_secs(2), notifications.recv())
        .await
        .expect("timeout")
        .expect("recv");
    assert_eq!(note.method, "heartbeat");
    assert_eq!(note.params, json!({"kind": "beat"}));
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_initiated_request_roundtrip() {
    let child = std::sync::Arc::new(JsonRpcChild::spawn(config()).await.expect("spawn"));
    let mut incoming = child.incoming_requests().expect("first claim");

    // Kick off a `server_request` from our side. The helper will respond
    // only after we answer its server-initiated `need_input` request.
    let outer_req = tokio::spawn({
        let c = std::sync::Arc::clone(&child);
        async move {
            c.request(
                "server_request",
                json!({
                    "server_id": "srv-42",
                    "payload": {"ask": "permission?"}
                }),
            )
            .await
        }
    });

    // We should see an incoming request with method "need_input".
    let req = tokio::time::timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("incoming timeout")
        .expect("incoming channel closed");
    assert_eq!(req.method, "need_input");
    assert_eq!(req.params, json!({"ask": "permission?"}));

    // Respond with a success payload. The helper then wraps that in its
    // reply to our original `server_request` call as `{"server_reply": ...}`.
    child
        .respond(req.id, Ok(json!({"approved": true})))
        .await
        .expect("respond");

    let reply = outer_req.await.expect("join").expect("outer request");
    assert_eq!(reply, json!({"server_reply": {"approved": true}}));
    drop(child);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_timeout() {
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    let result = child
        .request_with_timeout("sleep", json!({"ms": 1500}), Duration::from_millis(150))
        .await;
    match result {
        Err(RpcChildError::Timeout { method, elapsed }) => {
            assert_eq!(method, "sleep");
            assert_eq!(elapsed, Duration::from_millis(150));
        }
        other => panic!("expected Timeout, got {other:?}"),
    }
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_exit_fails_pending() {
    let child = std::sync::Arc::new(JsonRpcChild::spawn(config()).await.expect("spawn"));
    // Kick off a slow request that will still be pending when the child
    // dies.
    let slow = tokio::spawn({
        let c = std::sync::Arc::clone(&child);
        async move {
            c.request_with_timeout(
                "sleep",
                json!({"ms": 10_000}),
                Duration::from_secs(30),
            )
            .await
        }
    });

    // Give the slow request a moment to register.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Ask the helper to exit non-zero after writing stderr.
    child
        .notify(
            "exit",
            json!({"code": 7, "stderr": "farewell-diagnostic"}),
        )
        .await
        .expect("notify exit");

    let outcome = slow.await.expect("join");
    match outcome {
        Err(RpcChildError::RpcError(err)) => {
            // The watchdog synthesises an RpcError wrapping the exit info
            // when it cancels pending requests.
            assert!(
                err.message.contains("exited"),
                "message should mention exit: {}",
                err.message
            );
            assert!(
                err.message.contains("farewell-diagnostic"),
                "message should include stderr tail: {}",
                err.message
            );
        }
        Err(RpcChildError::ChildExited { code, stderr_tail }) => {
            assert_eq!(code, Some(7));
            assert!(stderr_tail.contains("farewell-diagnostic"));
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    // Subsequent requests should also fail fast.
    let after = child.request("echo", json!({"x": 1})).await;
    match after {
        Err(RpcChildError::ChildExited { code, .. }) => {
            assert_eq!(code, Some(7));
        }
        Err(RpcChildError::AlreadyShutdown) => {
            // Acceptable if exit-info has not propagated yet.
        }
        other => panic!("expected ChildExited, got {other:?}"),
    }
    drop(child);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_written_just_before_exit_is_routed_not_dropped() {
    // Exit-drain guarantee: the child answers the request and exits
    // IMMEDIATELY — the response bytes may still be sitting in the kernel
    // pipe buffer when the watchdog's `child.wait()` returns. The watchdog
    // must wait for the reader to drain stdout to EOF before failing
    // pending requests, so this request resolves with the real result, not
    // "child process exited". (Regression guard for the CI-observed race
    // where the watchdog won and the reader later logged the orphaned
    // response as an unknown id.)
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    let result = child
        .request("echo_then_exit", json!({"final": true}))
        .await
        .expect("response written before exit must be routed");
    assert_eq!(result, json!({"final": true}));

    // The child is gone; the handle must observe the exit promptly (the
    // drain must not hang once EOF has been reached).
    tokio::time::timeout(Duration::from_secs(3), async {
        while child.is_alive() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("exit must be observed promptly after the drain");
    let after = child.request("echo", json!(1)).await;
    assert!(
        matches!(
            after,
            Err(RpcChildError::ChildExited { .. }) | Err(RpcChildError::AlreadyShutdown)
        ),
        "post-exit request must fail fast, got: {after:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_is_graceful() {
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    assert!(child.is_alive());
    let result = child.request("echo", json!("ping")).await.expect("echo");
    assert_eq!(result, json!("ping"));
    // Helper exits cleanly once stdin closes.
    // Wait for the helper to acknowledge EOF and exit.
    tokio::time::timeout(Duration::from_secs(3), async {
        let h = JsonRpcChild::spawn(config()).await.expect("spawn");
        h.shutdown().await
    })
    .await
    .expect("shutdown should not hang")
    .expect("shutdown ok");

    // shutdown takes &self, so the original handle is still usable after
    // the call — though further RPCs will fail with ChildExited /
    // AlreadyShutdown. A second shutdown call must be a cheap no-op.
    let _ = child.shutdown().await;
    let second = child.shutdown().await;
    assert!(second.is_ok(), "second shutdown must be idempotent Ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_json_protocol_error() {
    // Helper's `malformed` method emits a bad line then a valid response.
    // The helper must not crash and the valid response must still land.
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    let result = child
        .request("malformed", json!({}))
        .await
        .expect("malformed roundtrip");
    assert_eq!(result, json!("after-malformed"));
    // A second request confirms the reader task is still alive after
    // skipping the malformed line.
    let result2 = child.request("echo", json!("still-here")).await.expect("echo");
    assert_eq!(result2, json!("still-here"));
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_requests() {
    let child = std::sync::Arc::new(JsonRpcChild::spawn(config()).await.expect("spawn"));

    let mut joins = Vec::new();
    for i in 0..20u64 {
        let c = std::sync::Arc::clone(&child);
        joins.push(tokio::spawn(async move {
            let v: Value = c
                .request("echo", json!({"i": i}))
                .await
                .expect("echo");
            assert_eq!(v, json!({"i": i}));
            i
        }));
    }

    let mut results = Vec::new();
    for j in joins {
        results.push(j.await.expect("join"));
    }
    results.sort();
    assert_eq!(results, (0..20u64).collect::<Vec<_>>());

    // Drop the Arc so we can shutdown cleanly.
    drop(child);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_error_from_child_is_surfaced() {
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    let result = child
        .request(
            "echo_err",
            json!({"code": -32602, "message": "bad params", "data": {"what": "everything"}}),
        )
        .await;
    match result {
        Err(RpcChildError::RpcError(err)) => {
            assert_eq!(err.code, -32602);
            assert_eq!(err.message, "bad params");
            assert_eq!(err.data, Some(json!({"what": "everything"})));
        }
        other => panic!("expected RpcError, got {other:?}"),
    }
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_requests_can_only_be_claimed_once() {
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    let first = child.incoming_requests();
    let second = child.incoming_requests();
    assert!(first.is_some(), "first claim should return Some");
    assert!(second.is_none(), "second claim must be None");
    let _ = child.shutdown().await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_waits_for_reap_even_when_stdout_closes_first() {
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: "python3".into(),
        args: vec![
            "-u".into(),
            "-c".into(),
            "import json,os,sys,time; q=json.loads(sys.stdin.readline()); print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':os.getpid()}),flush=True); os.close(1); time.sleep(60)".into(),
        ],
        ..config()
    })
    .await
    .expect("spawn owned peer");
    let pid = child.request("pid", json!({})).await.unwrap().as_u64().unwrap();
    let proc = PathBuf::from(format!("/proc/{pid}"));
    tokio::time::timeout(Duration::from_secs(2), async {
        while child.is_alive() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("stdout EOF should be observed");
    child.shutdown().await.expect("shutdown");
    let reaped_before_return = !proc.exists();
    // Keep the original failure deterministic without abandoning the owned peer.
    tokio::time::timeout(Duration::from_secs(5), async {
        while proc.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("watchdog must ultimately reap its peer");
    assert!(reaped_before_return, "stdout EOF is not proof that the owned process exited and was reaped before shutdown returned");
}
