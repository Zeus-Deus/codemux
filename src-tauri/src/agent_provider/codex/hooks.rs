//! Native hook discovery and the same config writes used by Codex's /hooks UI.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

use crate::json_rpc_child::JsonRpcChild;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HooksList {
    pub cwd: String,
    pub hooks: Vec<Value>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub errors: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum HookUpdate {
    Trust {
        key: String,
        hash: String,
    },
    SetEnabled {
        key: String,
        hash: String,
        enabled: bool,
    },
}

pub async fn list(child: &JsonRpcChild, cwd: &Path) -> Result<HooksList, String> {
    let response = child
        .request("hooks/list", json!({"cwds": [cwd]}))
        .await
        .map_err(|error| format!("Codex hook discovery failed: {error}"))?;
    let entry = response
        .get("data")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["cwd"].as_str() == cwd.to_str())
        })
        .ok_or_else(|| "Codex returned no hook catalogue for this directory.".to_string())?;
    serde_json::from_value(entry.clone())
        .map_err(|error| format!("Invalid Codex hook catalogue: {error}"))
}

pub fn update_params(catalogue: &HooksList, update: &HookUpdate) -> Result<Value, String> {
    let (key, hash) = match update {
        HookUpdate::Trust { key, hash } | HookUpdate::SetEnabled { key, hash, .. } => (key, hash),
    };
    let hook = catalogue
        .hooks
        .iter()
        .find(|hook| hook["key"].as_str() == Some(key))
        .ok_or_else(|| "This hook is no longer present. Refresh the hooks list.".to_string())?;
    if hook["isManaged"] != false {
        return Err("Managed hooks are controlled by your administrator.".into());
    }
    if hash.is_empty() || hook["currentHash"].as_str() != Some(hash) {
        return Err(
            "This hook changed since you reviewed it. Refresh and review its current definition."
                .into(),
        );
    }
    let state = match update {
        HookUpdate::Trust { .. } => json!({"trusted_hash": hash}),
        HookUpdate::SetEnabled { enabled, .. } => {
            if *enabled && hook["trustStatus"] != "trusted" {
                return Err("Review and trust this hook before enabling it.".into());
            }
            json!({"enabled": enabled})
        }
    };
    // Upsert the literal key as an object member; keys contain paths/dots.
    // Never turn it into a dotted config key or replace the whole state table.
    Ok(json!({
        "edits": [{"keyPath": "hooks.state", "value": {key: state}, "mergeStrategy": "upsert"}],
        "reloadUserConfig": true,
    }))
}

pub async fn manage(
    child: &JsonRpcChild,
    cwd: &Path,
    update: Option<HookUpdate>,
) -> Result<HooksList, String> {
    let catalogue = list(child, cwd).await?;
    let Some(update) = update else {
        return Ok(catalogue);
    };
    let params = update_params(&catalogue, &update)?;
    child
        .request("config/batchWrite", params)
        .await
        .map_err(|error| format!("Codex could not update this hook: {error}"))?;
    list(child, cwd).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalogue() -> HooksList {
        HooksList {
            cwd: "/repo".into(),
            hooks: vec![json!({
                "key": "path:/repo/.codex/hooks.json:Stop:0", "currentHash": "reviewed-hash",
                "isManaged": false, "trustStatus": "untrusted"
            })],
            warnings: vec![],
            errors: vec![],
        }
    }
    #[test]
    fn native_hooks_require_exact_review_and_preserve_literal_keys() {
        let catalogue = catalogue();
        let key = catalogue.hooks[0]["key"].as_str().unwrap().to_string();
        let update = HookUpdate::Trust {
            key: key.clone(),
            hash: "reviewed-hash".into(),
        };
        let params = update_params(&catalogue, &update).unwrap();
        assert_eq!(
            params["edits"][0]["value"],
            json!({key.clone(): {"trusted_hash": "reviewed-hash"}})
        );
        assert_eq!(params["edits"][0]["mergeStrategy"], "upsert");
        assert_eq!(params["reloadUserConfig"], true);
        assert!(update_params(
            &catalogue,
            &HookUpdate::Trust {
                key: key.clone(),
                hash: "old-hash".into()
            }
        )
        .is_err());
        assert!(update_params(
            &catalogue,
            &HookUpdate::SetEnabled {
                key: key.clone(),
                hash: "reviewed-hash".into(),
                enabled: true
            }
        )
        .is_err());
        assert!(update_params(
            &catalogue,
            &HookUpdate::SetEnabled {
                key,
                hash: "reviewed-hash".into(),
                enabled: false
            }
        )
        .is_ok());
        let mut managed = catalogue;
        managed.hooks[0]["isManaged"] = json!(true);
        assert!(update_params(&managed, &update).is_err());
    }
}
