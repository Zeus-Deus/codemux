//! Host-owned tool authority for an isolated workflow attempt.
//!
//! Registration cannot arrive over IPC. Adapters capture this context before
//! starting a session and never fall back to the unrestricted MCP registry.

use super::ThreadId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[async_trait]
pub trait ManagedToolHandler: Send + Sync {
    fn tools(&self) -> Vec<ManagedTool>;
    /// The implementation captures and reauthorizes its attempt. Arguments
    /// are untrusted and must never determine the caller's run or attempt.
    async fn call(&self, name: &str, arguments: Value) -> Result<Value, String>;
    /// Runtime-only callback; never part of the model's tool surface.
    fn record_runtime(&self, _evidence: Value) -> Result<(), String> {
        Err("managed runtime evidence recorder unavailable".into())
    }
    async fn quiesce(&self) -> Result<(), String> {
        Err("managed host-tool quiescence unavailable".into())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ManagedCapabilities {
    pub scoped_tools: bool,
    pub native_fanout_disabled: bool,
    pub enforced_read_only: bool,
    pub isolated_writes: bool,
    pub verified_stop: bool,
}

#[derive(Clone)]
pub struct ManagedSession {
    pub handler: Arc<dyn ManagedToolHandler>,
    pub read_only: bool,
}

impl ManagedSession {
    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value, String> {
        // A provider may send arbitrary names, including a removed tool.
        if !self.handler.tools().iter().any(|tool| tool.name == name) {
            return Err(format!("tool {name} is unavailable to this attempt"));
        }
        self.handler.call(name, arguments).await
    }
}

fn sessions() -> &'static Mutex<HashMap<ThreadId, Arc<ManagedSession>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<ThreadId, Arc<ManagedSession>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct ManagedSessionGuard {
    thread_id: ThreadId,
    session: Arc<ManagedSession>,
}

pub fn register_session(
    thread_id: ThreadId,
    session: ManagedSession,
) -> Result<ManagedSessionGuard, String> {
    let mut registry = sessions()
        .lock()
        .map_err(|_| "managed session registry unavailable")?;
    if registry.contains_key(&thread_id) {
        return Err("managed session already registered".into());
    }
    let session = Arc::new(session);
    registry.insert(thread_id.clone(), Arc::clone(&session));
    Ok(ManagedSessionGuard { thread_id, session })
}

pub fn lookup_session(thread_id: &ThreadId) -> Option<Arc<ManagedSession>> {
    sessions().lock().ok()?.get(thread_id).cloned()
}

impl Drop for ManagedSessionGuard {
    fn drop(&mut self) {
        if let Ok(mut registry) = sessions().lock() {
            if registry
                .get(&self.thread_id)
                .is_some_and(|value| Arc::ptr_eq(value, &self.session))
            {
                registry.remove(&self.thread_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Tools;
    #[async_trait]
    impl ManagedToolHandler for Tools {
        fn tools(&self) -> Vec<ManagedTool> {
            vec![ManagedTool {
                name: "result".into(),
                description: "result".into(),
                input_schema: serde_json::json!({"type":"object"}),
            }]
        }
        async fn call(&self, _: &str, arguments: Value) -> Result<Value, String> {
            Ok(arguments)
        }
    }
    #[tokio::test]
    async fn managed_authority_is_host_owned_and_removed_on_drop() {
        let id = ThreadId(uuid::Uuid::new_v4().to_string());
        let guard = register_session(
            id.clone(),
            ManagedSession {
                handler: Arc::new(Tools),
                read_only: true,
            },
        )
        .unwrap();
        let context = lookup_session(&id).unwrap();
        assert!(context
            .call("unrestricted_registry_tool", Value::Null)
            .await
            .is_err());
        assert!(context
            .call("result", serde_json::json!({"caller_attempt":"forged"}))
            .await
            .is_ok());
        assert!(register_session(
            id.clone(),
            ManagedSession {
                handler: Arc::new(Tools),
                read_only: true
            }
        )
        .is_err());
        drop(guard);
        assert!(lookup_session(&id).is_none());
    }
}
