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
    if let Some(remote) = spawn.remote.as_ref() {
        #[cfg(unix)] {
            let program = crate::ssh::sidecar::device_program(remote, &spawn.codex_binary).await?;
            let config = crate::ssh::sidecar::device_spawn_config(remote, &program,
                &["app-server".into()], &env, cwd, DEFAULT_RPC_TIMEOUT)?;
            return JsonRpcChild::spawn(config).await.map(Arc::new)
                .map_err(crate::ssh::sidecar::ssh_spawn_error);
        }
        #[cfg(not(unix))] {
            let _ = remote;
            return Err(ProviderError::ValidationError {
                message: "Threads on another device need macOS or Linux on this computer".into(),
            });
        }
    }
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
            self.ensure_open().await?;
            let Some(managed) = self.chatgpt_runtime.as_ref() else {
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
                    self.retire_managed_child().await?;
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
            let (thread, model, service_tier) = {
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
                    state.service_tier.clone(),
                )
            };
            self.retire_managed_child().await?;
            self.dead.store(true, Ordering::Release);
            let spawned = if let Some(context) = self.managed.as_ref() {
                spawn_managed_codex(&managed.spawn, &managed.env, &self.cwd, context, Some(&grant), Some(lease)).await
            } else {
                spawn_child(&managed.spawn, &self.cwd, managed.env.clone(), Some(&grant), Some(lease)).await
            };
            let child = match spawned {
                Ok(child) => child,
                Err(error) => {
                    self.runtime_lease.lock().await.take();
                    return Err(error);
                }
            };
            let initialized: Result<(), ProviderError> = async {
                if self.managed.is_none() {
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
                }
                let entries = read_model_catalog(&child)
                    .await
                    .map_err(|message| ProviderError::RpcError { message })?;
                selected_model(&entries, model.as_deref())?;
                let service_tier = resolve_catalog_service_tier(&entries, model.as_deref(), &service_tier);
                let (approval_policy, sandbox) =
                    codex_permission_mode_to_policy_pair(managed.permission.as_deref())
                        .map(|(a, s)| (Some(a), Some(s)))
                        .unwrap_or((None, None));
                let mut params = serde_json::to_value(ThreadResumeParams {
                            thread_id: thread.clone(),
                            model,
                            service_tier: Some((service_tier != "default").then(|| service_tier.clone())),
                            cwd: Some(self.cwd.clone()),
                            collaboration_mode: None,
                            approval_policy,
                            sandbox,
                            experimental_raw_events: false,
                        }).unwrap();
                if self.managed.is_some() { restrict_managed_codex_environment(&mut params); }
                let response = child.request("thread/resume", params).await
                    .map_err(|e| ProviderError::RpcError {
                        message: format!("Codex renewal could not resume the original thread: {e}"),
                    })?;
                if self.managed.is_some() {
                    validate_managed_codex_thread(&response)
                        .map_err(|message| ProviderError::ValidationError { message })?;
                }
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
                    .iter()
                    .filter(|m| !m.hidden)
                    .map(|m| m.id.clone())
                    .collect();
                let mut state = self.state.lock().await;
                state.tier_models = entries;
                state.service_tier = service_tier;
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

    async fn retire_managed_child(&self) -> Result<(), ProviderError> {
        let _ = self.shutdown_tx.send(());
        let tasks = std::mem::take(&mut *self.tasks.lock().await);
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        let child = self.child().await;
        let stopped = if self.managed.is_some() { child.shutdown_managed().await }
            else { child.shutdown_owned().await };
        stopped.map_err(|error| ProviderError::ProcessError {
            message: "Codex child retirement could not be confirmed".into(),
            source: Some(error.to_string()),
        })
    }
}
