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
    let (mut args, env) = if let Some(grant) = grant {
        grant.launch(env)
    } else {
        if let Some(home) = spawn.codex_home.as_ref() {
            env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
        }
        (vec!["app-server".into()], env)
    };
    switch_off_user_codemux_server(&mut args, &env);
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

/// The MCP server name the guide gives Codemux when setting up an outside agent.
const USER_CODEMUX_SERVER: &str = "codemux";

/// A regular in-app session reads the user's `config.toml`, so a user-level
/// Codemux MCP server (set up for outside agents) would run inside the chat
/// with the owner's reach, around the chat's own ceiling. The chat already
/// gets Codemux's tools as host tools, so the session switches those entries
/// off. Only declared entries are overridden, so a session without one starts
/// exactly as before.
fn switch_off_user_codemux_server(args: &mut Vec<String>, env: &HashMap<String, String>) {
    let home = env
        .get("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| crate::provider_accounts::homes::default_home(crate::agent_provider::ProviderKind::Codex));
    let Some(config) = home.and_then(|home| std::fs::read_to_string(home.join("config.toml")).ok()) else { return };
    let own = crate::mcp_server::codemux_mcp_command();
    if let Some(off) = mcp_servers_off_override(&user_codemux_servers(&config, std::path::Path::new(&own))) {
        args.extend(["-c".into(), off]);
    }
}

/// The top-level `[mcp_servers.*]` entries of a Codex `config.toml` that are
/// Codemux: each one that runs `codemux mcp`, under any name, and the one
/// named `codemux` (the guide's name, which a `--url` entry for the network
/// door also uses). `own` is this app's executable, as Settings offers it.
fn user_codemux_servers(config_toml: &str, own: &std::path::Path) -> Vec<String> {
    let Ok(config) = toml::from_str::<toml::Table>(config_toml) else { return Vec::new() };
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else { return Vec::new() };
    servers
        .iter()
        .filter(|(name, entry)| name.as_str() == USER_CODEMUX_SERVER || runs_codemux_mcp(entry, own))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Whether an entry runs a Codemux executable whose command line parses to
/// `codemux mcp`, so root flags before it count (`codemux --client codex mcp`).
fn runs_codemux_mcp(entry: &toml::Value, own: &std::path::Path) -> bool {
    use clap::Parser;
    let Some(command) = entry.get("command").and_then(toml::Value::as_str) else { return false };
    let args: Option<Vec<&str>> = match entry.get("args") {
        None => Some(Vec::new()),
        Some(args) => args.as_array().and_then(|args| args.iter().map(toml::Value::as_str).collect()),
    };
    let Some(args) = args else { return false };
    is_codemux_executable(command, own)
        && crate::cli::Cli::try_parse_from(std::iter::once("codemux").chain(args))
            .is_ok_and(|cli| matches!(cli.command, Some(crate::cli::CommandSet::Mcp { .. })))
}

/// `codemux` by name on any platform (`codemux.exe`, an install path), or
/// this app's own executable under another name (an AppImage).
fn is_codemux_executable(command: &str, own: &std::path::Path) -> bool {
    let path = std::path::Path::new(command);
    let named = path.file_stem().and_then(|stem| stem.to_str()).is_some_and(|stem| stem.eq_ignore_ascii_case("codemux"));
    named || std::fs::canonicalize(path).is_ok_and(|path| std::fs::canonicalize(own).is_ok_and(|own| own == path))
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

#[cfg(test)]
mod user_codemux_server_tests {
    use super::*;

    #[test]
    fn user_codemux_servers_reads_only_the_mcp_servers_table() {
        let own = std::path::Path::new("/nonexistent/own-codemux");
        for (config, servers) in [
            ("[mcp_servers.codemux]\ncommand = \"codemux\"\nargs = [\"mcp\"]\n", &["codemux"][..]),
            ("[mcp_servers.\"codemux\"]\ncommand = \"codemux\"\n", &["codemux"]),
            ("mcp_servers = { codemux = { command = \"codemux\" } }\n", &["codemux"]),
            ("[mcp_servers.codemux]\nenabled = false\n", &["codemux"]),
            ("[mcp_servers.codemux]\nurl = \"http://127.0.0.1:4378/mcp\"\n", &["codemux"]),
            ("[mcp_servers.other]\ncommand = \"codemux\"\n", &[]),
            ("[mcp_servers.codemux-dev]\ncommand = \"codemux\"\n", &[]),
            ("[profiles.work.mcp_servers.codemux]\ncommand = \"codemux\"\n", &[]),
            ("# [mcp_servers.codemux]\nmodel = \"codemux\"\n", &[]),
            ("mcp_servers = \"codemux\"\n", &[]),
            ("[mcp_servers.codemux\n", &[]),
            ("", &[]),
        ] {
            assert_eq!(user_codemux_servers(config, own), servers, "{config:?}");
        }
    }

    /// By what it runs: a Codemux executable whose command line is `mcp`.
    #[test]
    fn user_codemux_servers_finds_codemux_mcp_under_any_name() {
        let dir = tempfile::tempdir().unwrap();
        // An AppImage, which Settings offers by its own path.
        let own = dir.path().join("Codemux_1.0.0_amd64.AppImage");
        std::fs::write(&own, "").unwrap();
        let entry = |command: &str, args: &str| format!("[mcp_servers.x]\ncommand = {command:?}\nargs = {args}\n");
        for (command, args, found) in [
            ("codemux", r#"["mcp"]"#, true),
            ("codemux", r#"["mcp", "--client", "codex", "--access", "supervised"]"#, true),
            ("codemux", r#"["--client", "codex", "mcp"]"#, true),
            ("codemux", r#"["--access=read-only", "--disable-plugins", "mcp"]"#, true),
            ("/usr/local/bin/codemux", r#"["mcp"]"#, true),
            ("Codemux.exe", r#"["mcp"]"#, true),
            (own.to_str().unwrap(), r#"["mcp"]"#, true),
            ("codemux", "[]", false),
            ("codemux", r#"["mcp-call", "--list"]"#, false),
            ("codemux", r#"["notify", "mcp"]"#, false),
            ("codemux", r#"["mcp", "--no-such-flag"]"#, false),
            ("codemux", r#"["mcp", 1]"#, false),
            ("codemux-remote", r#"["mcp"]"#, false),
            ("ssh", r#"["-T", "desktop", "codemux", "mcp"]"#, false),
            ("npx", r#"["codemux", "mcp"]"#, false),
            (dir.path().join("other.AppImage").to_str().unwrap(), r#"["mcp"]"#, false),
        ] {
            let config = entry(command, args);
            assert_eq!(!user_codemux_servers(&config, &own).is_empty(), found, "{config}");
        }
        let config = entry("codemux", r#"["--client", "codex", "mcp"]"#);
        assert_eq!(user_codemux_servers(&format!("{config}[mcp_servers.codemux]\nurl = \"u\"\n"), &own), ["codemux", "x"]);
    }

    #[test]
    fn in_app_codex_switches_off_a_user_codemux_server() {
        let home = tempfile::tempdir().unwrap();
        let env = HashMap::from([("CODEX_HOME".to_string(), home.path().to_string_lossy().into_owned())]);
        let mut args = vec!["app-server".to_string()];
        switch_off_user_codemux_server(&mut args, &env);
        assert_eq!(args, ["app-server"], "no config: nothing to switch off");
        std::fs::write(home.path().join("config.toml"), "[mcp_servers.other]\ncommand = \"x\"\n").unwrap();
        switch_off_user_codemux_server(&mut args, &env);
        assert_eq!(args, ["app-server"], "only a declared codemux entry is overridden");
        std::fs::write(home.path().join("config.toml"), "[mcp_servers.codemux]\ncommand = \"codemux\"\nargs = [\"mcp\"]\n").unwrap();
        switch_off_user_codemux_server(&mut args, &env);
        assert_eq!(args[..2], ["app-server", "-c"]);
        assert_eq!(switched_off(&args[2]), ["codemux"]);
        assert_eq!(args.len(), 3);
        // Codex splits a `-c` key on every `.` and keeps quote characters, so
        // the key is the bare table name and every server name is in the value.
        assert_eq!(args[2].split_once('=').unwrap().0, "mcp_servers");
    }

    /// The server names an `mcp_servers={…}` override switches off, checking
    /// that it says only `enabled = false` for each.
    fn switched_off(arg: &str) -> Vec<String> {
        let parsed: toml::Table = toml::from_str(arg).unwrap_or_else(|error| panic!("{arg}: {error}"));
        let servers = parsed["mcp_servers"].as_table().unwrap();
        for (name, entry) in servers {
            assert_eq!(entry.to_string(), "{ enabled = false }", "{name} in {arg}");
        }
        servers.keys().cloned().collect()
    }

    /// `codex mcp add cmux -- codemux mcp` must not load inside in-app chats
    /// either: what an entry runs decides, not what it is called.
    #[test]
    fn in_app_codex_switches_off_codemux_mcp_under_any_name() {
        let home = tempfile::tempdir().unwrap();
        let env = HashMap::from([("CODEX_HOME".to_string(), home.path().to_string_lossy().into_owned())]);
        let config = "[mcp_servers.cmux]\ncommand = \"codemux\"\nargs = [\"mcp\", \"--client\", \"codex\"]\n\n\
                      [mcp_servers.tools]\ncommand = \"/usr/bin/codemux\"\nargs = [\"--client\", \"codex\", \"mcp\"]\n\n\
                      [mcp_servers.other]\ncommand = \"codemux\"\nargs = [\"notify\"]\n";
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        let mut args = vec!["app-server".to_string()];
        switch_off_user_codemux_server(&mut args, &env);
        assert_eq!(args[..2], ["app-server", "-c"]);
        assert_eq!(switched_off(&args[2]), ["cmux", "tools"], "one flag for every entry");
        assert_eq!(args.len(), 3);
    }

    /// The real Codex must take both switch-off overrides (the in-app one and
    /// the workflow restart's) and turn off exactly the named entries: argv
    /// comparisons alone once passed a form Codex refuses to start with.
    /// Skipped when no `codex` on PATH can list its MCP servers as JSON.
    #[test]
    fn real_codex_accepts_the_mcp_switch_off_overrides() {
        let Ok(codex) = which::which("codex") else {
            eprintln!("skipped: codex is not on PATH");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let config = "[mcp_servers.cmux]\ncommand = \"codemux\"\nargs = [\"mcp\"]\n\n\
                      [mcp_servers.\"dotted.name\"]\ncommand = \"codemux\"\nargs = [\"--client\", \"codex\", \"mcp\"]\n\n\
                      [mcp_servers.other]\ncommand = \"echo\"\nargs = [\"x\"]\n";
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        // A cleared env plus what a launcher needs, with this throwaway home.
        let list = |args: &[String]| {
            std::process::Command::new(&codex)
                .args(args)
                .args(["mcp", "list", "--json"])
                .env_clear()
                .envs(["PATH", "HOME"].into_iter().filter_map(|key| Some((key, std::env::var_os(key)?))))
                .env("CODEX_HOME", home.path())
                .current_dir(home.path())
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap()
        };
        let enabled = |output: &std::process::Output, name: &str| -> Option<bool> {
            let servers: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).ok()?;
            servers.iter().find(|server| server["name"] == name)?["enabled"].as_bool()
        };
        let baseline = list(&[]);
        if !baseline.status.success() || enabled(&baseline, "cmux") != Some(true) {
            eprintln!("skipped: {} cannot list MCP servers as JSON", codex.display());
            return;
        }

        let env = HashMap::from([("CODEX_HOME".to_string(), home.path().to_string_lossy().into_owned())]);
        let mut in_app = Vec::new();
        switch_off_user_codemux_server(&mut in_app, &env);
        let mut workflow = managed_codex_args(&["cmux".into(), "dotted.name".into()]);
        assert_eq!(workflow.pop().as_deref(), Some("app-server"));
        for args in [in_app, workflow] {
            let output = list(&args);
            assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
            for (name, on) in [("cmux", false), ("dotted.name", false), ("other", true)] {
                assert_eq!(enabled(&output, name), Some(on), "{name} with {args:?}: {}", String::from_utf8_lossy(&output.stdout));
            }
        }
    }
}
