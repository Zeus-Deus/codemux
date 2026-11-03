use super::*;
use serde_json::json;

fn request(message: Value) -> Request {
    match parse(message) {
        Ok(Incoming::Request(request)) => request,
        other => panic!("expected a request, got {other:?}"),
    }
}

fn notification(message: Value) -> Notification {
    match parse(message) {
        Ok(Incoming::Notification(notification)) => notification,
        other => panic!("expected a notification, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

#[test]
fn negotiate_echoes_each_supported_version() {
    for version in SUPPORTED_PROTOCOL_VERSIONS {
        assert_eq!(negotiate(Some(version)), version);
    }
    assert_eq!(LATEST_PROTOCOL_VERSION, "2025-11-25");
}

#[test]
fn negotiate_answers_latest_for_unknown_and_missing_offers() {
    for offer in [None, Some("2030-01-01"), Some("wrong"), Some("2025-6-18"), Some(""), Some(" 2025-06-18")] {
        assert_eq!(negotiate(offer), "2025-11-25", "{offer:?}");
    }
}

#[test]
fn initialize_never_negotiates_the_stateless_2026_07_28_revision() {
    assert_eq!(negotiate(Some("2026-07-28")), "2025-11-25");
    assert!(!SUPPORTED_PROTOCOL_VERSIONS.contains(&"2026-07-28"));
    let offer = InitializeRequest::parse(&json!({ "protocolVersion": "2026-07-28" }));
    assert_eq!(offer.negotiated_version(), "2025-11-25");
}

#[test]
fn http_version_defaults_missing_header_to_2025_03_26() {
    assert_eq!(http_version(None), Ok("2025-03-26"));
    assert_eq!(HTTP_DEFAULT_PROTOCOL_VERSION, "2025-03-26");
    for version in SUPPORTED_PROTOCOL_VERSIONS {
        assert_eq!(http_version(Some(version)), Ok(version));
    }
}

#[test]
fn http_version_rejects_unsupported_and_modern_headers() {
    for header in ["2026-07-28", "null", "", "2025-11-25 ", "wrong"] {
        let error = http_version(Some(header)).unwrap_err();
        assert_eq!(error.requested, header);
        let rpc = error.rpc_error();
        assert_eq!(rpc.code, -32600, "a modern-era -32022 would stop clients falling back");
        assert!(rpc.message.contains("Unsupported MCP-Protocol-Version"), "{}", rpc.message);
        assert!(rpc.message.contains("2025-11-25, 2025-06-18, 2025-03-26, 2024-11-05"), "{}", rpc.message);
    }
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

#[test]
fn parse_rejects_batches() {
    let error = parse(json!([{ "jsonrpc": "2.0", "id": 1, "method": "ping" }])).unwrap_err();
    assert_eq!(error.code, -32600);
    assert!(error.message.contains("Batch"), "{}", error.message);
    assert_eq!(parse(json!([])).unwrap_err().code, -32600);
}

#[test]
fn parse_rejects_non_objects() {
    for message in [json!("ping"), json!(1), json!(null), json!(true)] {
        assert_eq!(parse(message.clone()).unwrap_err().code, -32600, "{message}");
    }
}

#[test]
fn parse_rejects_null_float_bool_and_object_ids() {
    for id in [json!(null), json!(1.5), json!(1.0), json!(true), json!({ "n": 1 }), json!([1])] {
        let error = parse(json!({ "jsonrpc": "2.0", "id": id, "method": "ping" })).unwrap_err();
        assert_eq!(error.code, -32600, "{id}");
    }
}

#[test]
fn parse_rejects_bad_methods_and_params() {
    for message in [
        json!({ "jsonrpc": "2.0", "id": 1, "method": 7 }),
        json!({ "jsonrpc": "2.0", "id": 1 }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "ping", "params": [1] }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "ping", "params": "x" }),
    ] {
        assert_eq!(parse(message.clone()).unwrap_err().code, -32600, "{message}");
    }
}

#[test]
fn request_ids_serialize_back_unchanged() {
    for (id, line) in [
        (json!(7), "7"),
        (json!("7"), "\"7\""),
        (json!(u64::MAX), "18446744073709551615"),
        (json!(-1), "-1"),
        (json!("2b1f1d36-uuid"), "\"2b1f1d36-uuid\""),
    ] {
        let request = request(json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }));
        let reply = Response::result(request.id, json!({})).to_line();
        assert_eq!(reply, format!("{{\"jsonrpc\":\"2.0\",\"id\":{line},\"result\":{{}}}}"));
    }
    assert_ne!(RequestId::from_value(&json!(1)), RequestId::from_value(&json!("1")));
    assert_eq!(serde_json::from_value::<RequestId>(json!(-1)).unwrap(), RequestId::Int(-1));
    assert_eq!(serde_json::from_value::<RequestId>(json!(u64::MAX)).unwrap(), RequestId::Uint(u64::MAX));
}

#[test]
fn lenient_parse_accepts_missing_jsonrpc_and_null_params() {
    let parsed = request(json!({ "id": 1, "method": "tools/list" }));
    assert_eq!(parsed, Request { id: RequestId::Int(1), method: "tools/list".into(), params: json!({}) });
    let parsed = request(json!({ "jsonrpc": "2.0", "id": "a", "method": "ping", "params": null }));
    assert_eq!(parsed.params, json!({}));
    // A wrong jsonrpc value is tolerated too: local clients are trusted to be sloppy.
    let parsed = request(json!({ "jsonrpc": "1.0", "id": 2, "method": "ping" }));
    assert_eq!(parsed.method, "ping");
}

#[test]
fn parse_keeps_request_meta() {
    let meta = json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28", "progressToken": 3 });
    let parsed = request(json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta, "cursor": null }
    }));
    assert_eq!(parsed.params["_meta"], meta, "a per-request version must stay readable");
}

#[test]
fn lenient_parse_ignores_client_responses_and_id_less_requests() {
    assert_eq!(parse(json!({ "jsonrpc": "2.0", "id": 4, "result": {} })), Ok(Incoming::Response));
    assert_eq!(parse(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": 1 } })), Ok(Incoming::Response));
    assert_eq!(parse(json!({ "id": "x", "error": "odd" })), Ok(Incoming::Response));
    assert_eq!(
        notification(json!({ "jsonrpc": "2.0", "method": "tools/call", "params": { "name": "x" } })),
        Notification::Other("tools/call".into()),
        "a request without an id is never executed",
    );
}

#[test]
fn parse_reads_initialized_and_cancelled_notifications() {
    assert_eq!(notification(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })), Notification::Initialized);
    assert_eq!(
        notification(json!({
            "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": "r-1", "reason": "user" }
        })),
        Notification::Cancelled { request_id: RequestId::Str("r-1".into()), reason: Some("user".into()) },
    );
    assert_eq!(
        notification(json!({ "method": "notifications/cancelled", "params": { "requestId": 9 } })),
        Notification::Cancelled { request_id: RequestId::Int(9), reason: None },
    );
    for params in [json!({}), json!({ "requestId": null }), json!({ "requestId": 1.5 })] {
        assert_eq!(
            notification(json!({ "method": "notifications/cancelled", "params": params })),
            Notification::Other("notifications/cancelled".into()),
        );
    }
    assert_eq!(
        notification(json!({ "method": "notifications/roots/list_changed" })),
        Notification::Other("notifications/roots/list_changed".into()),
    );
}

#[test]
fn response_shapes_follow_json_rpc() {
    let success = Response::result(RequestId::Int(1), json!({ "hello": "world" })).to_value();
    assert_eq!(success, json!({ "jsonrpc": "2.0", "id": 1, "result": { "hello": "world" } }));
    let error = Response::error(Some(RequestId::Int(2)), RpcError::invalid_request("Bad request")).to_value();
    assert_eq!(error, json!({ "jsonrpc": "2.0", "id": 2, "error": { "code": -32600, "message": "Bad request" } }));
    assert!(!Response::error(None, RpcError::method_not_found("x")).to_line().contains("\"data\""));
    let envelope = Response::error(None, RpcError::parse_error("eof")).to_value();
    assert_eq!(envelope["id"], Value::Null);
    assert_eq!(envelope["error"]["message"], "Parse error: eof");
    let with_data = RpcError { data: Some(json!({ "supported": ["2025-11-25"] })), ..RpcError::invalid_params("x") };
    assert_eq!(Response::error(None, with_data).to_value()["error"]["data"]["supported"][0], "2025-11-25");
    assert_eq!(
        Response::from_outcome(RequestId::Str("a".into()), Err(RpcError::internal("x"))).to_value()["error"]["code"],
        -32603
    );
    assert_eq!(RpcError::method_not_found("server/discover").message, "Method not found: server/discover");
}

#[test]
fn response_lines_never_contain_newlines() {
    let line = Response::result(RequestId::Int(1), json!({ "text": "one\ntwo\r\nthree" })).to_line();
    assert!(!line.contains('\n') && !line.contains('\r'), "{line}");
    let back: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(back["result"]["text"], "one\ntwo\r\nthree");
}

// ---------------------------------------------------------------------------
// Initialize
// ---------------------------------------------------------------------------

#[test]
fn lenient_initialize_tolerates_empty_and_odd_params() {
    for params in [json!({}), json!(null), json!([]), json!({ "protocolVersion": 2025, "clientInfo": "me" })] {
        let offer = InitializeRequest::parse(&params);
        assert_eq!(offer, InitializeRequest::default(), "{params}");
        assert_eq!(offer.negotiated_version(), LATEST_PROTOCOL_VERSION);
    }
}

#[test]
fn initialize_reads_client_info_and_ignores_extra_keys() {
    let offer = InitializeRequest::parse(&json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "roots": {} },
        "clientInfo": {
            "name": "Hermes Agent", "version": "2.0.0", "title": "Hermes",
            "description": "a bot", "icons": [{ "src": "x.png" }], "websiteUrl": "https://example.com"
        },
        "_meta": {}
    }));
    assert_eq!(offer.negotiated_version(), "2024-11-05");
    assert_eq!(
        offer.client_info,
        Some(ClientInfo { name: "Hermes Agent".into(), version: "2.0.0".into(), title: Some("Hermes".into()) })
    );
    let bare = InitializeRequest::parse(&json!({ "clientInfo": { "name": "vexis" } }));
    assert_eq!(bare.client_info, Some(ClientInfo { name: "vexis".into(), version: String::new(), title: None }));
    for client_info in [json!({ "name": "" }), json!({ "version": "1" }), json!({ "name": 3 }), json!("claude")] {
        assert_eq!(InitializeRequest::parse(&json!({ "clientInfo": client_info })).client_info, None);
    }
}

#[test]
fn initialize_result_shapes_per_server() {
    let stdio = InitializeResult {
        protocol_version: "2025-11-25",
        tools_list_changed: None,
        server_info: ServerInfo { name: "codemux", version: "1.2.3" },
        instructions: Some("Use the tools."),
    };
    assert_eq!(
        stdio.to_value(),
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "codemux", "version": "1.2.3" },
            "instructions": "Use the tools."
        })
    );
    let remote = InitializeResult {
        protocol_version: "2024-11-05",
        tools_list_changed: Some(false),
        server_info: ServerInfo { name: "codemux-remote", version: "1.2.3" },
        instructions: None,
    };
    assert_eq!(
        remote.to_value(),
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "codemux-remote", "version": "1.2.3" }
        })
    );
    let listed = InitializeResult { tools_list_changed: Some(true), ..remote };
    assert_eq!(listed.to_value()["capabilities"]["tools"]["listChanged"], true);
}

// ---------------------------------------------------------------------------
// Tool calls and results
// ---------------------------------------------------------------------------

#[test]
fn call_tool_params_default_arguments_and_reject_non_objects() {
    let call = CallToolParams::parse(&json!({ "name": "thread_list" })).unwrap();
    assert_eq!(call, CallToolParams { name: "thread_list".into(), arguments: json!({}), meta: None });
    let call = CallToolParams::parse(&json!({ "name": "x", "arguments": null, "_meta": "ignored" })).unwrap();
    assert_eq!((call.arguments, call.meta), (json!({}), None));
    let meta = json!({ "codemux/workspace_id": "ws-1" });
    let call = CallToolParams::parse(&json!({ "name": "x", "arguments": { "a": 1 }, "_meta": meta })).unwrap();
    assert_eq!((call.arguments, call.meta), (json!({ "a": 1 }), Some(meta)));

    for params in [json!({}), json!({ "name": "" }), json!({ "name": 7 }), json!(null)] {
        let error = CallToolParams::parse(&params).unwrap_err();
        assert_eq!(error.code, -32602, "{params}");
        assert_eq!(error.message, "tools/call requires params.name");
    }
    for arguments in [json!("x"), json!([1]), json!(1)] {
        let error = CallToolParams::parse(&json!({ "name": "x", "arguments": arguments })).unwrap_err();
        assert_eq!(error.code, -32602, "{arguments}");
    }
}

#[test]
fn tool_result_text_has_no_is_error_key() {
    let line = text_result("x").to_string();
    assert!(!line.contains("isError"), "{line}");
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), json!({ "content": [{ "type": "text", "text": "x" }] }));
}

#[test]
fn tool_result_shapes_match_existing_wire_formats() {
    assert_eq!(error_result("boom"), json!({ "content": [{ "type": "text", "text": "boom" }], "isError": true }));
    assert_eq!(text_with_status("ok", false), json!({ "content": [{ "type": "text", "text": "ok" }], "isError": false }));
    assert_eq!(text_with_status("no", true), error_result("no"));
    // The stdio server's legacy success shape (no isError) and its failure shape.
    assert_eq!(text_result("done"), json!({ "content": [{ "type": "text", "text": "done" }] }));
}
