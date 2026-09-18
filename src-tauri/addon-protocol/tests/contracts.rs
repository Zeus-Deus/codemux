use codemux_addon_protocol::{
    limits::{self, RateLimit},
    manifest::Platform,
    wire::{read_frame, Envelope},
    Manifest,
};
use serde_json::json;
use std::{
    io::{BufReader, Cursor},
    time::{Duration, Instant},
};
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("../fixtures/hello.json")).unwrap()
}
#[test]
fn rejects_unknown_capabilities_and_invalid_declarations() {
    let good = fixture();
    assert!(Manifest::parse(
        &serde_json::to_vec(&good).unwrap(),
        Some(Platform::LinuxX64)
    )
    .is_ok());
    for (key, value) in [
        ("permissions", json!(["shell.execute"])),
        ("entry", json!("other.js")),
        ("api", json!("^2.0.0")),
        ("platforms", json!(["macos-arm64"])),
        ("extra", json!(true)),
        ("id", json!("root../bad")),
    ] {
        let mut bad = good.clone();
        bad[key] = value;
        assert!(
            Manifest::parse(&serde_json::to_vec(&bad).unwrap(), None).is_err(),
            "{key}"
        );
    }
    let mut bad = good;
    bad["http"] = json!([{"origin":"https://api.github.com","methods":["GET"]}]);
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap(), None).is_err());
    bad["http"][0]["credential"] = serde_json::Value::Null;
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap(), None).is_ok());
    bad["contributes"]["commands"] = json!([{"id":"hello","title":"First","requiresWorkspace":false},{"id":"hello","title":"Second","requiresWorkspace":false}]);
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap(), None).is_err());
}
#[test]
fn frames_are_bounded_before_newline_and_require_complete_utf8_json() {
    let mut input = BufReader::with_capacity(7, Cursor::new(vec![b'a'; limits::FRAME + 1]));
    assert!(read_frame(&mut input).is_err());
    assert!(read_frame(&mut Cursor::new(b"partial")).is_err());
    let mut input = Cursor::new(b"{}\n[]\n");
    assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"{}");
    assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"[]");
    assert!(read_frame(&mut input).unwrap().is_none());
}
#[test]
fn direction_generation_and_response_shape_are_enforced() {
    let message =
        json!({"jsonrpc":"2.0","generation":"current","id":1,"method":"host.request","params":{}});
    let bytes = serde_json::to_vec(&message).unwrap();
    assert!(Envelope::parse(&bytes, Some("current"), true).is_ok());
    assert!(Envelope::parse(&bytes, Some("stale"), true).is_err());
    assert!(Envelope::parse(&bytes, Some("current"), false).is_err());
    let acknowledgement = serde_json::to_vec(&json!({
        "jsonrpc":"2.0", "generation":"current", "id":2, "method":"ui.ack",
        "params":{"viewId":"view", "revision":1}
    }))
    .unwrap();
    assert!(Envelope::parse(&acknowledgement, Some("current"), false).is_ok());
    assert!(Envelope::parse(&acknowledgement, Some("current"), true).is_err());
    assert!(Envelope::parse(&acknowledgement, Some("stale"), false).is_err());
    for extra in [
        json!({"result":null}),
        json!({"id":"1"}),
        json!({"pluginId":"forged"}),
        json!({"method":"invoke"}),
    ] {
        let mut bad = message.clone();
        bad.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(
            Envelope::parse(&serde_json::to_vec(&bad).unwrap(), Some("current"), true).is_err()
        );
    }
}
#[test]
fn quotas_allow_bursts_but_never_exceed_rolling_windows() {
    let mut limit = RateLimit::default();
    let start = Instant::now();
    for second in 0..5 {
        for _ in 0..20 {
            assert!(limit.accept(start + Duration::from_secs(second), 20, 100));
        }
        assert!(!limit.accept(start + Duration::from_secs(second), 20, 100));
    }
    assert!(!limit.accept(start + Duration::from_secs(59), 20, 100));
    assert!(limit.accept(start + Duration::from_secs(60), 20, 100));
}
