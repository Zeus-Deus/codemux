//! Token-bound Codex child rebuilding. All callers hold the session's outbound gate.
use super::super::chatgpt::{ManagedGrant, Owner};
use super::*;

pub(super) struct ManagedSession {
    pub owner: Arc<Owner>,
    pub spawn: CodexSpawnConfig,
    pub env: HashMap<String, String>,
    pub revision: Mutex<String>,
    pub permission: Option<String>,
    pub models: Mutex<Vec<String>>,
}

pub(super) async fn spawn_child(
    spawn: &CodexSpawnConfig,
    cwd: &std::path::Path,
    mut env: HashMap<String, String>,
    grant: Option<&ManagedGrant>,
    lease: Option<Arc<std::fs::File>>,
) -> Result<Arc<JsonRpcChild>, ProviderError> {
    let (args, env) = if let Some(grant) = grant {
        grant.launch(env)
    } else {
        if let Some(home) = spawn.codex_home.as_ref() {
            env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
        }
        (vec!["app-server".into()], env)
    };
    let config = SpawnConfig {
        program: spawn.codex_binary.clone(),
        args,
        env,
        cwd: Some(cwd.into()),
        default_timeout: DEFAULT_RPC_TIMEOUT,
    };
    let child = match lease {
        Some(lease) => JsonRpcChild::spawn_owned(config, grant.is_some(), lease).await,
        None => {
            if grant.is_some() {
                JsonRpcChild::spawn_isolated(config).await
            } else {
                JsonRpcChild::spawn(config).await
            }
        }
    };
    child
        .map(Arc::new)
        .map_err(|e| ProviderError::ProcessError {
            message: "Cannot start Codex app-server".into(),
            source: Some(e.to_string()),
        })
}

/// Preserve opaque IDs and ordering, including all pages. No bundled/default fallback here.
pub(crate) async fn read_model_catalog(
    child: &JsonRpcChild,
) -> Result<Vec<super::super::protocol::ModelEntry>, String> {
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    for _ in 0..32 {
        let response = child
            .request(
                "model/list",
                serde_json::to_value(super::super::protocol::ModelListParams {
                    cursor,
                    ..Default::default()
                })
                .unwrap(),
            )
            .await
            .map_err(|e| format!("Codex model catalog could not be read: {e}"))?;
        let page: super::super::protocol::ModelListResponse = serde_json::from_value(response)
            .map_err(|_| "Codex returned an invalid model catalog")?;
        models.extend(page.data);
        match page.next_cursor {
            None => return Ok(models),
            Some(next) if seen.insert(next.clone()) => cursor = Some(next),
            Some(_) => return Err("Codex repeated a model catalog cursor".into()),
        }
    }
    Err("Codex model catalog exceeded its page limit".into())
}

pub(super) fn selected_model(
    entries: &[super::super::protocol::ModelEntry],
    selected: Option<&str>,
) -> Result<String, ProviderError> {
    let selected = selected
        .and_then(|id| entries.iter().find(|m| !m.hidden && m.id == id))
        .or_else(|| {
            if selected.is_none() {
                entries
                    .iter()
                    .find(|m| !m.hidden && m.is_default)
                    .or_else(|| entries.iter().find(|m| !m.hidden))
            } else {
                None
            }
        });
    selected.map(|m|m.id.clone()).ok_or_else(||ProviderError::ValidationError { message: "Select a model from the current ChatGPT plan catalog. No available model was selected.".into() })
}

impl CodexSession {
    // The boxed boundary also prevents an opaque-future recursion through the
    // restarted notification pump -> queue drain -> child renewal.
    pub(super) fn ensure_managed_child(
        self: &Arc<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ProviderError>> + Send + '_>>
    {
        Box::pin(async move {
            let Some(managed) = self.managed.as_ref() else {
                return Ok(());
            };
            let lease = {
                let mut slot = self.runtime_lease.lock().await;
                if slot.is_none() {
                    let admitted = managed
                        .owner
                        .acquire_runtime()
                        .map_err(|message| ProviderError::ValidationError { message })?;
                    *slot = Some(Arc::new(admitted));
                }
                slot.as_ref().unwrap().clone()
            };
            let grant = match managed.owner.usable().await {
                Ok(Some(grant)) => grant,
                result => {
                    self.retire_managed_child().await;
                    self.dead.store(true, Ordering::Release);
                    self.runtime_lease.lock().await.take();
                    return Err(ProviderError::NotAuthenticated {
                        provider: crate::agent_provider::ProviderKind::Codex,
                        hint: result
                            .err()
                            .unwrap_or_else(|| "Reconnect ChatGPT to continue this chat".into()),
                    });
                }
            };
            let mut revision = managed.revision.lock().await;
            if *revision == grant.revision
                && !self.dead.load(Ordering::Acquire)
                && self.child().await.is_alive()
            {
                return Ok(());
            }
            let (thread, model, fast) = {
                let state = self.state.lock().await;
                if state.active_turn.is_some() || !state.pending_approvals.is_empty() {
                    return Err(ProviderError::ValidationError {
                        message:
                            "Finish the active Codex turn and approvals before renewing its child."
                                .into(),
                    });
                }
                (
                    state.codex_thread_id.clone(),
                    state.model.clone(),
                    state.fast_mode,
                )
            };
            self.retire_managed_child().await;
            self.dead.store(true, Ordering::Release);
            let child = match spawn_child(
                &managed.spawn,
                &self.cwd,
                managed.env.clone(),
                Some(&grant),
                Some(lease),
            )
            .await
            {
                Ok(child) => child,
                Err(error) => {
                    self.runtime_lease.lock().await.take();
                    return Err(error);
                }
            };
            let initialized: Result<(), ProviderError> = async {
                child
                    .request(
                        "initialize",
                        serde_json::to_value(InitializeParams {
                            client_info: managed.spawn.client_info.clone(),
                            capabilities: Capabilities {
                                experimental_api: true,
                            },
                        })
                        .unwrap(),
                    )
                    .await
                    .map_err(|e| ProviderError::RpcError {
                        message: format!("Codex renewal initialize failed: {e}"),
                    })?;
                child.notify("initialized", json!({})).await.map_err(|e| {
                    ProviderError::RpcError {
                        message: format!("Codex renewal initialized failed: {e}"),
                    }
                })?;
                let entries = read_model_catalog(&child)
                    .await
                    .map_err(|message| ProviderError::RpcError { message })?;
                selected_model(&entries, model.as_deref())?;
                let (approval_policy, sandbox) =
                    codex_permission_mode_to_policy_pair(managed.permission.as_deref())
                        .map(|(a, s)| (Some(a), Some(s)))
                        .unwrap_or((None, None));
                let response = child
                    .request(
                        "thread/resume",
                        serde_json::to_value(ThreadResumeParams {
                            thread_id: thread.clone(),
                            model,
                            service_tier: Some(fast.then(|| "fast".into())),
                            cwd: Some(self.cwd.clone()),
                            collaboration_mode: None,
                            approval_policy,
                            sandbox,
                            experimental_raw_events: false,
                        })
                        .unwrap(),
                    )
                    .await
                    .map_err(|e| ProviderError::RpcError {
                        message: format!("Codex renewal could not resume the original thread: {e}"),
                    })?;
                let resumed: ThreadStartResponse =
                    serde_json::from_value(response).map_err(|_| ProviderError::RpcError {
                        message: "Codex renewal returned an invalid thread".into(),
                    })?;
                if resumed.thread_id() != thread {
                    return Err(ProviderError::ValidationError {
                        message: "Codex renewal did not resume the original thread".into(),
                    });
                }
                *managed.models.lock().await = entries
                    .into_iter()
                    .filter(|m| !m.hidden)
                    .map(|m| m.id)
                    .collect();
                Ok(())
            }
            .await;
            if let Err(error) = initialized {
                let _ = child.shutdown_owned().await;
                self.runtime_lease.lock().await.take();
                return Err(error);
            }
            let incoming =
                child
                    .incoming_requests()
                    .ok_or_else(|| ProviderError::ProcessError {
                        message: "Codex renewal lost its request channel".into(),
                        source: None,
                    })?;
            *self.child.write().await = child.clone();
            self.dead.store(false, Ordering::Release);
            let tasks = vec![
                spawn_notifications_task(
                    self.clone(),
                    child.clone(),
                    self.event_tx.clone(),
                    self.shutdown_tx.subscribe(),
                ),
                spawn_incoming_requests_task(
                    self.clone(),
                    child.clone(),
                    managed.spawn.mcp_registry.clone(),
                    incoming,
                    self.event_tx.clone(),
                    self.shutdown_tx.subscribe(),
                ),
                spawn_child_exit_watchdog(
                    self.clone(),
                    child,
                    self.event_tx.clone(),
                    self.shutdown_tx.subscribe(),
                ),
            ];
            self.tasks.lock().await.extend(tasks);
            *revision = grant.revision;
            Ok(())
        })
    }

    async fn retire_managed_child(&self) {
        let _ = self.shutdown_tx.send(());
        let tasks = std::mem::take(&mut *self.tasks.lock().await);
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        let _ = self.child().await.shutdown_owned().await;
    }
}
