use codemux_addon_protocol::{
    catalog::Catalog,
    limits::{self, RateLimit},
    manifest::Platform,
    wire::{read_frame, Envelope},
    ErrorCode, Manifest,
};
use serde_json::{json, Map, Value};
use std::{
    io::{BufReader, Cursor},
    path::Path,
    time::{Duration, Instant},
};
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("../fixtures/hello.json")).unwrap()
}
fn parent<'a>(manifest: &'a mut Value, pointer: &str) -> (&'a mut Map<String, Value>, String) {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    let parent = manifest.pointer_mut(parent).and_then(Value::as_object_mut);
    (parent.unwrap(), key.into())
}
/// Shared accept/reject cases for every manifest validator. A case replaces
/// `base` with `manifest` or edits it: `set` inserts values at object paths and
/// `remove` deletes keys. `error` is the desktop's message, or null when valid.
#[test]
fn shared_manifest_cases_match_the_desktop_validator() {
    let cases: Value =
        serde_json::from_str(include_str!("../fixtures/manifest-cases.json")).unwrap();
    assert_eq!(cases["base"], "hello.json");
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let mut manifest = case.get("manifest").cloned().unwrap_or_else(fixture);
        let set = case.get("set").and_then(Value::as_object);
        for (pointer, value) in set.into_iter().flatten() {
            let (object, key) = parent(&mut manifest, pointer);
            object.insert(key, value.clone());
        }
        let remove = case.get("remove").and_then(Value::as_array);
        for pointer in remove.into_iter().flatten() {
            let (object, key) = parent(&mut manifest, pointer.as_str().unwrap());
            assert!(object.remove(&key).is_some(), "{name}");
        }
        let result = Manifest::parse(&serde_json::to_vec(&manifest).unwrap(), None);
        assert_eq!(
            result.err().map(|e| e.message),
            case["error"].as_str().map(String::from),
            "{name}"
        );
    }
}
#[test]
fn manifest_size_api_and_platform_are_checked_before_use() {
    let mut padded = serde_json::to_vec(&fixture()).unwrap();
    padded.resize(limits::MANIFEST, b' ');
    assert!(Manifest::parse(&padded, None).is_ok());
    padded.push(b' ');
    assert_eq!(
        Manifest::parse(&padded, None).unwrap_err().message,
        "Manifest exceeds 64 KiB"
    );
    let mut manifest = fixture();
    manifest["api"] = json!("^2.0.0");
    let error = Manifest::parse(&serde_json::to_vec(&manifest).unwrap(), None).unwrap_err();
    assert_eq!(error.data.code, ErrorCode::IncompatibleApi);
    manifest["api"] = json!("^1.0.0");
    manifest["platforms"] = json!(["linux-x64"]);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    assert!(Manifest::parse(&bytes, Some(Platform::LinuxX64)).is_ok());
    assert_eq!(
        Manifest::parse(&bytes, Some(Platform::WindowsX64))
            .unwrap_err()
            .message,
        "Unsupported platform"
    );
}
#[test]
fn published_schemas_are_generated_from_the_rust_contracts() {
    let manifest = serde_json::to_value(schemars::schema_for!(Manifest)).unwrap();
    let catalog = serde_json::to_value(schemars::schema_for!(Catalog)).unwrap();
    for (path, generated) in [
        ("../../packages/plugin-sdk/schema/manifest.json", &manifest),
        ("../../packages/plugin-cli/schema/manifest.json", &manifest),
        ("../../catalog/addons/schema/catalog-v1.json", &catalog),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let committed: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            committed == *generated,
            "{} is stale; regenerate it with the addon-protocol schema examples",
            path.display()
        );
    }
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
