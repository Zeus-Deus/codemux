//! Outcome evidence from Codex app-server tool items. Fields/statuses are pinned
//! to `codex-cli 0.162.1 app-server generate-ts --experimental`; MCP `isError`
//! also accepts older native envelopes. Unknown statuses never prove success.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolOutcome {
    Success,
    Error,
    Unconfirmed,
}

/// Only interpret Codex's native tool variants. Web search has no outcome fields
/// in this protocol; arbitrary tool result JSON must not be interpreted here.
pub(crate) fn native_tool_outcome(item: &Value) -> Option<ToolOutcome> {
    let kind = item.get("type")?.as_str()?;
    if !matches!(
        kind,
        "commandExecution" | "fileChange" | "dynamicToolCall" | "mcpToolCall"
    ) {
        return None;
    }
    let status = item.get("status").filter(|value| !value.is_null());
    if status.and_then(Value::as_str) == Some("failed")
        // Codex uses declined for both approval rejection and cancellation.
        || (matches!(kind, "commandExecution" | "fileChange")
            && status.and_then(Value::as_str) == Some("declined"))
        || (kind == "commandExecution"
            && item.get("exitCode").and_then(Value::as_i64).is_some_and(|code| code != 0))
        || (kind == "dynamicToolCall" && item.get("success").and_then(Value::as_bool) == Some(false))
        || (kind == "mcpToolCall"
            && (item.get("error").is_some_and(|error| !error.is_null())
                || item.pointer("/result/isError").and_then(Value::as_bool) == Some(true)))
    {
        return Some(ToolOutcome::Error);
    }
    if status.and_then(Value::as_str) == Some("completed") {
        return Some(ToolOutcome::Success);
    }
    if status.is_some() {
        return Some(ToolOutcome::Unconfirmed);
    }
    // Older envelopes may lack status but still carry explicit outcome evidence.
    let succeeded = match kind {
        "commandExecution" => item.get("exitCode").and_then(Value::as_i64) == Some(0),
        "dynamicToolCall" => item.get("success").and_then(Value::as_bool) == Some(true),
        "mcpToolCall" => item.pointer("/result/isError").and_then(Value::as_bool) == Some(false),
        _ => false,
    };
    Some(if succeeded {
        ToolOutcome::Success
    } else {
        ToolOutcome::Unconfirmed
    })
}

/// A lazy history body must retain the evidence the reducer uses to correct old
/// is_error=false rows. Keep this bounded: never copy output, arguments or error
/// text. The original item remains in the database and is fetched on expansion.
/// False means the caller must keep the original body to avoid hiding evidence.
pub(crate) fn preserve_outcome_in_stub(item: &Value, stub: &mut Value) -> bool {
    let Some(outcome) = native_tool_outcome(item) else {
        return true;
    };
    let Some(id) = item
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.len() <= 256)
    else {
        return false;
    };
    let Some(stub) = stub.as_object_mut() else {
        return false;
    };
    let status = match outcome {
        ToolOutcome::Success => "done",
        ToolOutcome::Error => "error",
        ToolOutcome::Unconfirmed => "unconfirmed",
    };
    stub.insert(
        "__codemux_codex_tool_outcome".into(),
        serde_json::json!({
            "tool_use_id": id,
            "status": status,
        }),
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_tool_outcomes_stub_evidence_is_bounded() {
        let native = json!({
            "type": "mcpToolCall", "id": "mcp", "status": "failed",
            "error": {"message": "X".repeat(100_000)},
        });
        let mut stub = json!({"__codemux_lazy_tool_result": {"row_id": 1}});
        assert!(preserve_outcome_in_stub(&native, &mut stub));
        assert_eq!(
            stub["__codemux_codex_tool_outcome"],
            json!({
                "tool_use_id": "mcp", "status": "error",
            })
        );
        assert!(serde_json::to_string(&stub).unwrap().len() < 256);

        let mut oversized_id = native;
        oversized_id["id"] = json!("X".repeat(257));
        assert!(!preserve_outcome_in_stub(&oversized_id, &mut json!({})));
        let unrelated = json!({"status": "failed", "error": true});
        let mut stub = json!({});
        assert!(preserve_outcome_in_stub(&unrelated, &mut stub));
        assert_eq!(stub, json!({}));
    }
}
