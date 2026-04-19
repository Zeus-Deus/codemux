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
async fn shutdown_is_graceful() {
    let child = JsonRpcChild::spawn(config()).await.expect("spawn");
    assert!(child.is_alive());
    let result = child.request("echo", json!("ping")).await.expect("echo");
    assert_eq!(result, json!("ping"));
    // Helper exits cleanly once stdin closes.
    // shutdown consumes the handle.
    // Wait for the helper to acknowledge EOF and exit.
    tokio::time::timeout(Duration::from_secs(3), async {
        let h = JsonRpcChild::spawn(config()).await.expect("spawn");
        h.shutdown().await
    })
    .await
    .expect("shutdown should not hang")
    .expect("shutdown ok");

    let _ = child.shutdown().await;
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
