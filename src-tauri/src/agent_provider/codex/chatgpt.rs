//! Native-only ChatGPT grant owner. Never expose credential records through IPC.
use serde::Serialize;
use std::path::PathBuf;
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptStatus {
    pub phase: String,
    pub attempt_id: Option<String>,
    pub email: Option<String>,
    pub error: Option<String>,
    pub profiles: Vec<ChatGptProfile>,
    pub active_profile_id: Option<String>,
    pub welcome_pending: bool,
    pub installed: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptProfile {
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    pub connected: bool,
}
#[derive(Default, serde::Deserialize, Serialize)]
#[serde(default)]
struct Disk {
    local_workbench: bool,
    host_id: String,
    profiles: std::collections::BTreeMap<String, Registration>,
    sessions: std::collections::BTreeMap<String, Credentials>,
    pending_refresh: std::collections::BTreeMap<String, PendingRefresh>,
    active_profile_id: Option<String>,
    welcome_pending: bool,
    managed_selected: bool,
    epoch: u64,
}
#[derive(Clone, serde::Deserialize, Serialize)]
struct Registration {
    subject: String,
    email: Option<String>,
    label: String,
    welcomed: bool,
}
#[derive(Clone, serde::Deserialize, Serialize)]
struct Credentials {
    access_token: String,
    refresh_token: String,
    id_token: String,
    scopes: Vec<String>,
    expires_at: u64,
    earliest_refresh_at: u64,
    revision: String,
}
#[derive(Clone, serde::Deserialize, Serialize)]
struct PendingRefresh {
    credentials: Credentials,
    previous_revision: String,
    verify_identity: bool,
}
#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    token_type: String,
    expires_in: u64,
    scope: String,
    #[serde(default)]
    earliest_refresh_at: serde_json::Value,
}
#[derive(serde::Deserialize)]
struct Identity {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}
fn sharing(scopes: &[String]) -> bool {
    ["resource.invoke", "chatgpt.tokens.use.direct"]
        .iter()
        .all(|scope| scopes.iter().any(|s| s == scope))
}
pub struct ManagedGrant {
    pub(crate) access_token: String,
    pub(crate) revision: String,
    pub(crate) home: PathBuf,
    pub(crate) cache_key: String,
}
impl ManagedGrant {
    pub(crate) fn launch(
        &self,
        caller: std::collections::HashMap<String, String>,
    ) -> (Vec<String>, std::collections::HashMap<String, String>) {
        let mut env: std::collections::HashMap<String, String> = caller
            .into_iter()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "CODEMUX_WORKSPACE_ID"
                        | "CODEMUX_PANE_ID"
                        | "CODEMUX_HOOK_PORT"
                        | "CODEMUX_CONTROL_SOCKET"
                )
            })
            .collect();
        env.insert("ACCESS_TOKEN".into(), self.access_token.clone());
        env.insert(
            "CODEX_HOME".into(),
            self.home.to_string_lossy().into_owned(),
        );
        let args=vec!["app-server".into(),"--listen".into(),"stdio://".into(),"-c".into(),"model_provider=\"openai_chatgpt_plan\"".into(),"-c".into(),"model_providers={openai_chatgpt_plan={name=\"ChatGPT plan\",base_url=\"https://api.openai.com/v1\",env_key=\"ACCESS_TOKEN\",wire_api=\"responses\",requires_openai_auth=false,supports_websockets=false,model_catalog_url=\"https://api.openai.com/v1/models\"}}".into(),"-c".into(),"features.api_key_model_discovery=true".into()];
        (args, env)
    }
}
pub struct AttemptLaunch {
    pub url: String,
    pub status: ChatGptStatus,
}
struct Pending {
    id: String,
    state: String,
    nonce: String,
    verifier: String,
    redirect_uri: String,
    client_id: Option<String>,
    abort: tokio::sync::oneshot::Sender<()>,
    _admission: std::fs::File,
    expires: std::time::Instant,
}
const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, serde::Deserialize)]
struct Endpoints {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    revocation_endpoint: String,
}
#[derive(Clone)]
pub struct Owner {
    root: PathBuf,
    operation: std::sync::Arc<tokio::sync::Mutex<()>>,
    pending: std::sync::Arc<tokio::sync::Mutex<Option<Pending>>>,
    error: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    changed: tokio::sync::broadcast::Sender<()>,
    #[cfg(test)]
    fixture_endpoints: Option<Endpoints>,
}
fn random_secret() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
impl Owner {
    pub fn at(root: PathBuf) -> Self {
        Self {
            root,
            operation: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            pending: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            error: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            changed: tokio::sync::broadcast::channel(16).0,
            #[cfg(test)]
            fixture_endpoints: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn testing(root: PathBuf) -> std::sync::Arc<Self> {
        let value = Self::at(root);
        value.prepare().unwrap();
        let mut disk = Disk::default();
        disk.managed_selected = true;
        disk.active_profile_id = Some("oaiapp_fixture".into());
        disk.profiles.insert(
            "oaiapp_fixture".into(),
            Registration {
                subject: "fixture-subject".into(),
                email: None,
                label: "Fixture".into(),
                welcomed: true,
            },
        );
        disk.sessions.insert(
            "oaiapp_fixture".into(),
            Credentials {
                access_token: "synthetic-runtime-a".into(),
                refresh_token: "synthetic-refresh".into(),
                id_token: "synthetic-id".into(),
                scopes: vec!["resource.invoke".into(), "chatgpt.tokens.use.direct".into()],
                expires_at: now() + 3600,
                earliest_refresh_at: 0,
                revision: "revision-a".into(),
            },
        );
        value.write(&disk).unwrap();
        std::sync::Arc::new(value)
    }
    #[cfg(test)]
    pub(crate) async fn testing_rotate(&self) {
        let _file = self.disk_lock().await.unwrap();
        let mut disk = self.read().unwrap();
        let c = disk.sessions.get_mut("oaiapp_fixture").unwrap();
        c.access_token = "synthetic-runtime-b".into();
        c.revision = "revision-b".into();
        disk.epoch += 1;
        self.write(&disk).unwrap();
    }
    #[cfg(test)]
    pub(crate) async fn testing_revoke(&self) {
        let _file = self.disk_lock().await.unwrap();
        let mut disk = self.read().unwrap();
        disk.sessions.clear();
        disk.epoch += 1;
        self.write(&disk).unwrap();
    }
    pub fn changes(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.changed.subscribe()
    }
    fn invalidate(&self) {
        let _ = self.changed.send(());
    }
    fn prepare(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|_| "Cannot create private ChatGPT storage")?;
        let meta =
            std::fs::symlink_metadata(&self.root).map_err(|_| "Cannot inspect ChatGPT storage")?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err("ChatGPT storage must be a private directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "Cannot protect ChatGPT storage")?;
        }
        Ok(())
    }
    async fn disk_lock(&self) -> Result<std::fs::File, String> {
        self.prepare()?;
        let path = self.root.join("owner.lock");
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("Unsafe ChatGPT lock file".into());
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|_| "Cannot open ChatGPT owner lock")?;
        for _ in 0..200 {
            if file.try_lock().is_ok() {
                return Ok(file);
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        Err("ChatGPT storage is busy; try again".into())
    }
    fn read(&self) -> Result<Disk, String> {
        let path = self.root.join("state.json");
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Disk::default()),
            Ok(m) if m.file_type().is_symlink() || !m.is_file() => {
                return Err("Unsafe ChatGPT state file".into())
            }
            Err(_) => return Err("Cannot inspect ChatGPT state".into()),
            _ => {}
        }
        serde_json::from_slice(&std::fs::read(path).map_err(|_| "Cannot read ChatGPT state")?)
            .map_err(|_| "Cannot decode ChatGPT state".into())
    }
    fn write(&self, disk: &Disk) -> Result<(), String> {
        use std::io::Write;
        let path = self
            .root
            .join(format!(".state-{}.tmp", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options
                .open(&path)
                .map_err(|_| "Cannot write private ChatGPT state")?;
            let bytes = serde_json::to_vec(disk).map_err(|_| "Cannot encode ChatGPT state")?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "Cannot persist ChatGPT state")?;
            std::fs::rename(&path, self.root.join("state.json"))
                .map_err(|_| "Cannot publish ChatGPT state")?;
            #[cfg(unix)]
            std::fs::File::open(&self.root)
                .and_then(|f| f.sync_all())
                .map_err(|_| "Cannot sync ChatGPT storage")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        } else {
            self.invalidate();
        }
        result
    }
    pub async fn status(&self) -> Result<ChatGptStatus, String> {
        let initial = self.snapshot().await?;
        let ready = initial
            .profiles
            .iter()
            .any(|p| Some(&p.id) == initial.active_profile_id.as_ref() && p.connected);
        if initial.phase != "pending"
            && initial.active_profile_id.is_some()
            && (!ready || initial.error.is_some())
        {
            if let Err(message) = self.usable().await {
                *self.error.lock().await = Some(message);
            }
            return self.snapshot().await;
        }
        Ok(initial)
    }
    async fn snapshot(&self) -> Result<ChatGptStatus, String> {
        let _op = self.operation.lock().await;
        let _file = self.disk_lock().await?;
        let disk = self.read()?;
        let pending = self.pending.lock().await;
        let error = self.error.lock().await.clone();
        let active = disk
            .active_profile_id
            .as_ref()
            .and_then(|id| disk.sessions.get(id));
        let connected = disk
            .active_profile_id
            .as_ref()
            .is_some_and(|id| !disk.pending_refresh.contains_key(id))
            && active.is_some_and(|s| sharing(&s.scopes) && s.expires_at > now());
        let profiles = disk
            .profiles
            .iter()
            .map(|(id, p)| ChatGptProfile {
                id: id.clone(),
                label: p.label.clone(),
                email: p.email.clone(),
                connected: !disk.pending_refresh.contains_key(id)
                    && disk
                        .sessions
                        .get(id)
                        .is_some_and(|s| sharing(&s.scopes) && s.expires_at > now()),
            })
            .collect();
        Ok(ChatGptStatus {
            phase: if pending.is_some() {
                "pending"
            } else if connected {
                "connected"
            } else if error.is_some() {
                "error"
            } else {
                "disconnected"
            }
            .into(),
            attempt_id: pending.as_ref().map(|p| p.id.clone()),
            email: disk
                .active_profile_id
                .as_ref()
                .and_then(|id| disk.profiles.get(id))
                .and_then(|p| p.email.clone()),
            error,
            profiles,
            active_profile_id: disk.active_profile_id,
            welcome_pending: disk.welcome_pending && connected,
            installed: which::which("codex").is_ok(),
        })
    }
    pub async fn begin(
        self: &std::sync::Arc<Self>,
        profile: Option<String>,
    ) -> Result<AttemptLaunch, String> {
        use base64::Engine;
        use sha2::Digest;
        let _op = self.operation.lock().await;
        if self.pending.lock().await.is_some() {
            return Err("A ChatGPT sign-in is already pending".into());
        }
        let admission = self.runtime_lock(false)?;
        let _file = self.disk_lock().await?;
        let mut disk = self.read()?;
        if profile
            .as_ref()
            .is_some_and(|id| !disk.profiles.contains_key(id))
        {
            return Err("Saved ChatGPT account not found".into());
        }
        if disk.host_id.is_empty() {
            disk.host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
            self.write(&disk)?;
        }
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| "Cannot start ChatGPT loopback callback")?;
        let redirect_uri = format!(
            "http://127.0.0.1:{}/auth/callback",
            listener
                .local_addr()
                .map_err(|_| "Cannot inspect callback port")?
                .port()
        );
        let state = random_secret();
        let nonce = random_secret();
        let verifier = random_secret();
        let id = uuid::Uuid::new_v4().to_string();
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()));
        let mut url = url::Url::parse("https://auth.openai.com/api/accounts/authorize")
            .map_err(|_| "Invalid authorization endpoint")?;
        url.query_pairs_mut().extend_pairs([
            (
                "client_id",
                profile.as_deref().unwrap_or("dynamic_agent_client"),
            ),
            ("ext_agent_host_id", &disk.host_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect_uri),
            (
                "scope",
                "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
            ),
            ("resource", "https://api.openai.com/v1"),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge),
        ]);
        if let Some(client) = profile.as_ref() {
            if let Some(saved) = disk.sessions.get(client) {
                url.query_pairs_mut()
                    .append_pair("id_token_hint", &saved.id_token);
                if !sharing(&saved.scopes) {
                    url.query_pairs_mut().append_pair("prompt", "consent");
                }
            }
            if let Some(email) = disk.profiles.get(client).and_then(|p| p.email.as_deref()) {
                url.query_pairs_mut().append_pair("login_hint", email);
            }
        } else {
            url.query_pairs_mut()
                .append_pair("agent_name_hint", "codemux");
        }
        let (abort, mut aborted) = tokio::sync::oneshot::channel();
        *self.pending.lock().await = Some(Pending {
            id: id.clone(),
            state,
            nonce,
            verifier,
            redirect_uri,
            client_id: profile,
            abort,
            _admission: admission,
            expires: std::time::Instant::now() + std::time::Duration::from_secs(300),
        });
        *self.error.lock().await = None;
        self.invalidate();
        let owner = std::sync::Arc::clone(self);
        let attempt = id.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
            loop {
                let accepted = tokio::select! {
                    _=&mut aborted=>return,
                    _=tokio::time::sleep_until(deadline)=>{owner.finish_error(&attempt,"ChatGPT sign-in timed out. Try again.").await;return},
                    result=listener.accept()=>result,
                };
                let Ok((mut stream, _)) = accepted else {
                    owner
                        .finish_error(&attempt, "ChatGPT callback listener ended. Try again.")
                        .await;
                    return;
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0; 8192];
                let count = match tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    stream.read(&mut buf),
                )
                .await
                {
                    Ok(Ok(n)) => n,
                    _ => continue,
                };
                let target = std::str::from_utf8(&buf[..count])
                    .ok()
                    .and_then(|s| s.lines().next())
                    .and_then(|line| line.strip_prefix("GET "))
                    .and_then(|s| s.strip_suffix(" HTTP/1.1"));
                // Never drop an exchange on cancellation. The final attempt fence
                // decides publication; a late issued renewable grant is revoked.
                let accepted = match target {
                    Some(target) => owner.accept_callback(&attempt, target).await,
                    _ => false,
                };
                let (status, body) = if accepted {
                    (
                        "200 OK",
                        "ChatGPT sign-in received. Return to Codemux to check the connection.",
                    )
                } else {
                    (
                        "400 Bad Request",
                        "This callback does not match the pending sign-in.",
                    )
                };
                let response=format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{body}",body.len());
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    stream.write_all(response.as_bytes()),
                )
                .await;
                if accepted {
                    return;
                }
            }
        });
        drop(_file);
        drop(_op);
        Ok(AttemptLaunch {
            url: url.to_string(),
            status: self.status().await?,
        })
    }
    async fn finish_error(&self, id: &str, message: &str) {
        let mut pending = self.pending.lock().await;
        if pending.as_ref().is_some_and(|p| p.id == id) {
            pending.take();
            *self.error.lock().await = Some(message.into());
            self.invalidate();
        }
    }
    async fn accept_callback(&self, id: &str, target: &str) -> bool {
        let Some(query) = target.strip_prefix("/auth/callback?") else {
            return false;
        };
        if query.chars().any(|c| c.is_control()) || query.len() > 6000 {
            return false;
        }
        let mut params = std::collections::HashMap::new();
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if params
                .insert(key.into_owned(), value.into_owned())
                .is_some()
            {
                return false;
            }
        }
        let pending = self.pending.lock().await;
        let Some(p) = pending.as_ref().filter(|p| p.id == id) else {
            return false;
        };
        if params.get("state") != Some(&p.state) {
            return false;
        }
        if params.contains_key("error") {
            if params.contains_key("code") {
                return false;
            }
            drop(pending);
            self.finish_error(id, "ChatGPT sign-in was not authorized. Try again.")
                .await;
            return true;
        }
        if !params.get("code").is_some_and(|s| !s.is_empty()) {
            return false;
        }
        let client = params.get("client_id").or(p.client_id.as_ref());
        if !client
            .is_some_and(|c| valid_client(c) && p.client_id.as_ref().is_none_or(|old| old == c))
        {
            return false;
        }
        let client = client.unwrap().clone();
        let nonce = p.nonce.clone();
        let verifier = p.verifier.clone();
        let redirect = p.redirect_uri.clone();
        let expected = p.client_id.clone();
        let code = params["code"].clone();
        drop(pending);
        let mut issued_refresh = None;
        let result: Result<(), String> = async {
            let http = self.http()?;
            let endpoints = self.endpoints(&http).await?;
            let response = http
                .post(&endpoints.token_endpoint)
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", &client),
                    ("code", &code),
                    ("code_verifier", &verifier),
                    ("redirect_uri", &redirect),
                    ("resource", "https://api.openai.com/v1"),
                ])
                .send()
                .await
                .map_err(|_| "ChatGPT exchange could not be reached. Try again.")?;
            if response.status() != reqwest::StatusCode::OK {
                return Err("ChatGPT code exchange failed. Start a fresh sign-in.".into());
            }
            let tokens: TokenResponse = response
                .json()
                .await
                .map_err(|_| "ChatGPT token response was invalid")?;
            issued_refresh = Some(tokens.refresh_token.clone());
            let id_token = tokens
                .id_token
                .as_ref()
                .ok_or("ChatGPT identity token is missing")?;
            let identity = self
                .identity(&http, &endpoints, id_token, &client, Some(&nonce))
                .await?;
            let credentials = credentials(tokens, None)?;
            let _op = self.operation.lock().await;
            let _file = self.disk_lock().await?;
            let mut pending = self.pending.lock().await;
            if !pending
                .as_ref()
                .is_some_and(|p| p.id == id && p.expires > std::time::Instant::now())
            {
                return Err("ChatGPT attempt was cancelled or timed out".into());
            }
            let mut disk = self.read()?;
            if let Some(old) = disk.profiles.get(&client) {
                if old.subject != identity.sub {
                    return Err("The ChatGPT account did not match the saved registration".into());
                }
            } else if expected.is_some() {
                return Err("Saved ChatGPT account no longer exists".into());
            }
            let label = disk
                .profiles
                .get(&client)
                .map(|p| p.label.clone())
                .unwrap_or_else(|| format!("ChatGPT account {}", disk.profiles.len() + 1));
            let welcomed = disk.profiles.get(&client).is_some_and(|p| p.welcomed);
            disk.profiles.insert(
                client.clone(),
                Registration {
                    subject: identity.sub,
                    email: identity.email,
                    label,
                    welcomed,
                },
            );
            let enabled = sharing(&credentials.scopes);
            disk.pending_refresh.remove(&client);
            disk.sessions.insert(client.clone(), credentials);
            disk.active_profile_id = Some(client.clone());
            disk.managed_selected = true;
            disk.welcome_pending = enabled && !welcomed;
            disk.epoch = disk.epoch.wrapping_add(1);
            *self.error.lock().await = if enabled {
                None
            } else {
                Some(
                    "ChatGPT plan usage was not enabled. Reconnect this account to enable it."
                        .into(),
                )
            };
            self.write(&disk)?;
            pending.take();
            Ok(())
        }
        .await;
        if let Err(message) = result {
            if let Some(token) = issued_refresh {
                let _ = self.revoke(&client, &token).await;
            }
            self.finish_error(id, &message).await;
        }
        true
    }
    pub async fn cancel(&self, id: &str) -> Result<ChatGptStatus, String> {
        let mut pending = self.pending.lock().await;
        if !pending.as_ref().is_some_and(|p| p.id == id) {
            return Err("ChatGPT attempt no longer matches".into());
        }
        if let Some(p) = pending.take() {
            let _ = p.abort.send(());
        }
        drop(pending);
        *self.error.lock().await = None;
        self.invalidate();
        self.status().await
    }
    fn http(&self) -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|_| "Cannot prepare ChatGPT transport".into())
    }
    async fn endpoints(&self, http: &reqwest::Client) -> Result<Endpoints, String> {
        #[cfg(test)]
        if let Some(e) = &self.fixture_endpoints {
            return Ok(e.clone());
        }
        let response = http
            .get("https://auth.openai.com/.well-known/openid-configuration")
            .send()
            .await
            .map_err(|_| "Cannot discover ChatGPT sign-in")?;
        if response.status() != reqwest::StatusCode::OK {
            return Err("Cannot discover ChatGPT sign-in".into());
        }
        let e: Endpoints = response
            .json()
            .await
            .map_err(|_| "Invalid ChatGPT sign-in configuration")?;
        if e.issuer != ISSUER
            || e.authorization_endpoint != AUTHORIZE
            || e.token_endpoint != "https://auth.openai.com/api/accounts/oauth/token"
            || e.revocation_endpoint != "https://auth.openai.com/api/accounts/oauth/revoke"
            || e.jwks_uri != "https://auth.openai.com/.well-known/jwks.json"
        {
            return Err("ChatGPT sign-in configuration could not be verified".into());
        }
        Ok(e)
    }
    async fn identity(
        &self,
        http: &reqwest::Client,
        endpoints: &Endpoints,
        token: &str,
        client: &str,
        nonce: Option<&str>,
    ) -> Result<Identity, String> {
        let response = http
            .get(&endpoints.jwks_uri)
            .send()
            .await
            .map_err(|_| "Cannot verify ChatGPT identity")?;
        if response.status() != reqwest::StatusCode::OK {
            return Err("Cannot verify ChatGPT identity".into());
        }
        let keys: jsonwebtoken::jwk::JwkSet = response
            .json()
            .await
            .map_err(|_| "Invalid ChatGPT signing keys")?;
        verify_identity(token, &keys, client, nonce)
    }
    fn runtime_lock(&self, shared: bool) -> Result<std::fs::File, String> {
        self.prepare()?;
        let path = self.root.join("runtime.lock");
        if std::fs::symlink_metadata(&path)
            .is_ok_and(|m| m.file_type().is_symlink() || !m.is_file())
        {
            return Err("Unsafe ChatGPT runtime lock".into());
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|_| "Cannot open ChatGPT runtime lock")?;
        let result = if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        };
        result.map_err(|_| {
            if shared {
                "Finish or cancel ChatGPT sign-in before starting Codex"
            } else {
                "Stop all Codex chats before changing the ChatGPT connection"
            }
        })?;
        Ok(file)
    }
    pub fn acquire_runtime(&self) -> Result<std::fs::File, String> {
        self.runtime_lock(true)
    }
    pub async fn usable(&self) -> Result<Option<ManagedGrant>, String> {
        // Own rotation independently of the request waiting for it. Dropping a
        // renderer/launch request may not discard an issued refresh replacement.
        let owner = self.clone();
        tokio::spawn(async move { owner.usable_owned().await })
            .await
            .map_err(|_| "ChatGPT renewal task ended; try again".to_string())?
    }
    async fn usable_owned(&self) -> Result<Option<ManagedGrant>, String> {
        // Hold both the in-process owner and kernel file lock across rotation.
        // Every competing process re-reads the replacement, never its old token.
        let _op = self.operation.lock().await;
        let _file = self.disk_lock().await?;
        let mut disk = self.read()?;
        if !disk.managed_selected {
            return Ok(None);
        }
        let client = disk
            .active_profile_id
            .clone()
            .ok_or("Reconnect ChatGPT to use your plan")?;
        // A recovered candidate may already have expired while awaiting keys.
        // Publish its verified refresh credential, then renew from that latest
        // generation under these same locks before admitting inference.
        for _ in 0..2 {
            let previous = disk
                .sessions
                .get(&client)
                .cloned()
                .ok_or("Reconnect ChatGPT to use your plan")?;
            if !sharing(&previous.scopes) {
                return Err("ChatGPT plan usage is not enabled".into());
            }
            let current = now();
            let mut session = previous.clone();
            if disk.pending_refresh.contains_key(&client)
                || previous.expires_at <= current.saturating_add(60)
            {
                if !disk.pending_refresh.contains_key(&client)
                    && previous.earliest_refresh_at > current
                {
                    if previous.expires_at <= current {
                        return Err("ChatGPT renewal is not available yet. Try again later.".into());
                    }
                } else {
                    let renewed: Result<Credentials, String> = async {
                        let http = self.http()?;
                        let endpoints = self.endpoints(&http).await?;
                        if !disk.pending_refresh.contains_key(&client) {
                            let response = http
                                .post(&endpoints.token_endpoint)
                                .form(&[
                                    ("grant_type", "refresh_token"),
                                    ("client_id", client.as_str()),
                                    ("refresh_token", previous.refresh_token.as_str()),
                                    ("resource", "https://api.openai.com/v1"),
                                ])
                                .send()
                                .await
                                .map_err(|_| "ChatGPT renewal is temporarily unavailable")?;
                            if response.status() != reqwest::StatusCode::OK {
                                let status = response.status();
                                let body: serde_json::Value =
                                    response.json().await.unwrap_or_default();
                                let code = body
                                    .get("error")
                                    .and_then(|e| {
                                        e.as_str()
                                            .or_else(|| e.get("code").and_then(|v| v.as_str()))
                                    })
                                    .unwrap_or("");
                                if !status.is_server_error() && terminal_refresh(code) {
                                    disk.sessions.remove(&client);
                                    disk.welcome_pending = false;
                                    disk.epoch = disk.epoch.wrapping_add(1);
                                    self.write(&disk)?;
                                    *self.error.lock().await = Some(
                                    "ChatGPT authorization ended. Reconnect your saved account."
                                        .into(),
                                );
                                    return Err(
                                    "ChatGPT authorization ended. Reconnect your saved account."
                                        .into(),
                                );
                                }
                                return Err("ChatGPT renewal is temporarily unavailable".into());
                            }
                            let tokens: TokenResponse = response
                                .json()
                                .await
                                .map_err(|_| "ChatGPT renewal response was invalid")?;
                            let verify_identity = tokens.id_token.is_some();
                            let fresh = credentials(tokens, Some(&previous))?;
                            // Preserve the issued rotating replacement before any further
                            // networking. It is private and cannot authorize inference yet.
                            disk.pending_refresh.insert(
                                client.clone(),
                                PendingRefresh {
                                    credentials: fresh,
                                    previous_revision: previous.revision.clone(),
                                    verify_identity,
                                },
                            );
                            self.write(&disk)?;
                        }
                        let pending = disk
                            .pending_refresh
                            .get(&client)
                            .cloned()
                            .ok_or("ChatGPT renewal verification is missing")?;
                        if pending.previous_revision != previous.revision {
                            return Err(
                                "ChatGPT renewal no longer matches this account. Reconnect.".into(),
                            );
                        }
                        if pending.verify_identity {
                            let identity = self
                                .identity(
                                    &http,
                                    &endpoints,
                                    &pending.credentials.id_token,
                                    &client,
                                    None,
                                )
                                .await?;
                            if disk
                                .profiles
                                .get(&client)
                                .is_none_or(|p| p.subject != identity.sub)
                            {
                                return Err(
                                    "ChatGPT renewal identity did not match the saved account"
                                        .into(),
                                );
                            }
                        }
                        let fresh = pending.credentials;
                        if !sharing(&fresh.scopes) {
                            disk.pending_refresh.remove(&client);
                            disk.sessions.remove(&client);
                            disk.welcome_pending = false;
                            disk.epoch = disk.epoch.wrapping_add(1);
                            self.write(&disk)?;
                            return Err(
                                "ChatGPT plan permission ended. Reconnect your saved account."
                                    .into(),
                            );
                        }
                        Ok(fresh)
                    }
                    .await;
                    match renewed {
                        Ok(fresh) => {
                            session = fresh;
                            disk.pending_refresh.remove(&client);
                            disk.sessions.insert(client.clone(), session.clone());
                            disk.epoch = disk.epoch.wrapping_add(1);
                            self.write(&disk)?;
                            *self.error.lock().await = None;
                        }
                        Err(message) => {
                            // A transient failure must not destroy a usable token;
                            // a definitive removal must never fall back to it.
                            if disk.pending_refresh.contains_key(&client) {
                                *self.error.lock().await=Some("ChatGPT renewal verification is pending. Refresh the connection to retry.".into());
                                return Err(message);
                            }
                            if !disk.sessions.contains_key(&client) || previous.expires_at <= now()
                            {
                                return Err(message);
                            }
                        }
                    }
                }
            }
            if session.expires_at <= now() {
                continue;
            }
            let home = self.root.join("codex-home");
            std::fs::create_dir_all(&home).map_err(|_| "Cannot prepare managed Codex home")?;
            if std::fs::symlink_metadata(&home)
                .map_err(|_| "Cannot inspect managed Codex home")?
                .file_type()
                .is_symlink()
            {
                return Err("Unsafe managed Codex home".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| "Cannot protect managed Codex home")?;
            }
            let cache_key = format!("managed:{}:{}:{}", client, session.revision, disk.epoch);
            if session.expires_at <= now() {
                continue;
            }
            return Ok(Some(ManagedGrant {
                access_token: session.access_token,
                revision: session.revision,
                home,
                cache_key,
            }));
        }
        Err("ChatGPT credential expired during renewal. Refresh the connection to retry.".into())
    }
    // Atomic publication makes a read-only route check safe for synchronous
    // quota forwarding. Any unreadable state suppresses foreign quota data.
    pub(crate) fn managed_selected_now(&self) -> bool {
        self.read().map(|d| d.managed_selected).unwrap_or(true)
    }
    pub(crate) async fn cache_key(&self) -> Result<String, String> {
        let _op = self.operation.lock().await;
        let _file = self.disk_lock().await?;
        let disk = self.read()?;
        if !disk.managed_selected {
            return Ok(format!("legacy:{}", disk.epoch));
        }
        let client = disk
            .active_profile_id
            .as_ref()
            .ok_or("Reconnect ChatGPT to use your plan")?;
        let session = disk
            .sessions
            .get(client)
            .ok_or("Reconnect ChatGPT to use your plan")?;
        Ok(format!(
            "managed:{}:{}:{}",
            client, session.revision, disk.epoch
        ))
    }
    pub async fn disconnect(&self) -> Result<ChatGptStatus, String> {
        let _op = self.operation.lock().await;
        let _file = self.disk_lock().await?;
        let mut disk = self.read()?;
        if let Some(p) = self.pending.lock().await.take() {
            let _ = p.abort.send(());
        }
        let _admission = self.runtime_lock(false)?;
        let mut confirmed = true;
        if let Some(client) = disk.active_profile_id.as_ref() {
            if let Some(session) = disk
                .pending_refresh
                .get(client)
                .map(|p| &p.credentials)
                .or_else(|| disk.sessions.get(client))
            {
                confirmed = self.revoke(client, &session.refresh_token).await;
            }
        }
        if let Some(client) = disk.active_profile_id.take() {
            disk.pending_refresh.remove(&client);
            disk.sessions.remove(&client);
        }
        disk.managed_selected = false;
        disk.welcome_pending = false;
        disk.epoch = disk.epoch.wrapping_add(1);
        self.write(&disk)?;
        *self.error.lock().await = if confirmed {
            None
        } else {
            Some("Disconnected locally. Remote revocation was not confirmed; disconnect Codemux in ChatGPT Settings.".into())
        };
        drop(_file);
        drop(_op);
        self.status().await
    }
    async fn revoke(&self, client: &str, token: &str) -> bool {
        let Ok(http) = self.http() else { return false };
        let Ok(endpoints) = self.endpoints(&http).await else {
            return false;
        };
        for attempt in 0..3 {
            let response = http
                .post(&endpoints.revocation_endpoint)
                .form(&[
                    ("token", token),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", client),
                ])
                .send()
                .await;
            match response {
                Ok(r) if r.status() == reqwest::StatusCode::OK => return true,
                Ok(r) if !r.status().is_server_error() => return false,
                _ => {}
            }
            if attempt < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(200 << attempt)).await;
            }
        }
        false
    }
    pub async fn acknowledge_welcome(&self) -> Result<ChatGptStatus, String> {
        let _op = self.operation.lock().await;
        let _file = self.disk_lock().await?;
        let mut disk = self.read()?;
        if let Some(p) = disk
            .active_profile_id
            .as_ref()
            .and_then(|id| disk.profiles.get_mut(id))
        {
            p.welcomed = true;
        }
        disk.welcome_pending = false;
        self.write(&disk)?;
        drop(_file);
        drop(_op);
        self.status().await
    }
    pub async fn local_workbench(&self) -> Result<bool, String> {
        let _op = self.operation.lock().await;
        let _file = self.disk_lock().await?;
        Ok(self.read()?.local_workbench)
    }
    pub async fn set_local_workbench(&self, enabled: bool) -> Result<(), String> {
        let _op = self.operation.lock().await;
        let _file = self.disk_lock().await?;
        let mut disk = self.read()?;
        disk.local_workbench = enabled;
        self.write(&disk)
    }
}
pub fn owner() -> std::sync::Arc<Owner> {
    static OWNER: std::sync::OnceLock<std::sync::Arc<Owner>> = std::sync::OnceLock::new();
    OWNER
        .get_or_init(|| {
            std::sync::Arc::new(Owner::at(
                dirs::config_dir()
                    .unwrap_or_else(|| PathBuf::from(".config"))
                    .join(crate::APP_DIR_NAME)
                    .join("chatgpt"),
            ))
        })
        .clone()
}
fn verify_identity(
    token: &str,
    keys: &jsonwebtoken::jwk::JwkSet,
    client: &str,
    nonce: Option<&str>,
) -> Result<Identity, String> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation};
    let header = jsonwebtoken::decode_header(token).map_err(|_| "Invalid ChatGPT identity")?;
    if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
        return Err("Unsupported ChatGPT signing algorithm".into());
    }
    let jwk = header
        .kid
        .as_deref()
        .and_then(|kid| keys.find(kid))
        .ok_or("ChatGPT signing key not found")?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| "Invalid ChatGPT signing key")?;
    let mut validation = Validation::new(header.alg);
    validation.leeway = 0;
    validation.validate_nbf = true;
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client]);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
    let identity = jsonwebtoken::decode::<Identity>(token, &key, &validation)
        .map_err(|_| "ChatGPT identity could not be verified")?
        .claims;
    if identity.sub.is_empty() || nonce.is_some_and(|n| identity.nonce.as_deref() != Some(n)) {
        return Err("ChatGPT identity did not match this sign-in".into());
    }
    Ok(identity)
}
fn credentials(
    tokens: TokenResponse,
    previous: Option<&Credentials>,
) -> Result<Credentials, String> {
    if !tokens.token_type.eq_ignore_ascii_case("Bearer")
        || tokens.access_token.is_empty()
        || tokens.refresh_token.is_empty()
        || tokens.expires_in == 0
        || tokens.expires_in > 86400
    {
        return Err("Invalid ChatGPT token response".into());
    }
    let earliest = match tokens.earliest_refresh_at {
        serde_json::Value::Null => 0,
        serde_json::Value::Number(n) => n.as_u64().ok_or("Invalid ChatGPT refresh time")?,
        serde_json::Value::String(s) => chrono::DateTime::parse_from_rfc3339(&s)
            .map_err(|_| "Invalid ChatGPT refresh time")?
            .timestamp()
            .try_into()
            .map_err(|_| "Invalid ChatGPT refresh time")?,
        _ => return Err("Invalid ChatGPT refresh time".into()),
    };
    Ok(Credentials {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: tokens
            .id_token
            .or_else(|| previous.map(|c| c.id_token.clone()))
            .ok_or("ChatGPT identity token is missing")?,
        scopes: tokens.scope.split_whitespace().map(str::to_owned).collect(),
        expires_at: now().saturating_add(tokens.expires_in),
        earliest_refresh_at: earliest,
        revision: uuid::Uuid::new_v4().to_string(),
    })
}
fn terminal_refresh(code: &str) -> bool {
    matches!(
        code,
        "invalid_grant"
            | "invalid_client"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused"
    )
}
fn valid_client(c: &str) -> bool {
    c.starts_with("oaiapp_")
        && c.len() > 7
        && c.len() <= 200
        && c.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        endpoints: Endpoints,
        nonce: std::sync::Arc<tokio::sync::Mutex<String>>,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        revoked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        delay: std::sync::Arc<std::sync::atomic::AtomicBool>,
        seen: std::sync::Arc<tokio::sync::Notify>,
        release: std::sync::Arc<tokio::sync::Notify>,
        override_claims: std::sync::Arc<tokio::sync::Mutex<serde_json::Value>>,
        response: std::sync::Arc<tokio::sync::Mutex<Option<(u16, String)>>>,
        share: std::sync::Arc<std::sync::atomic::AtomicBool>,
        jwks_fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
        refresh_inputs: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn fixture() -> Fixture {
        use base64::Engine;
        let rsa = openssl::rsa::Rsa::generate(2048).unwrap();
        let pem = rsa.private_key_to_pem().unwrap();
        let jwk = serde_json::json!({"keys":[{"kty":"RSA","alg":"RS256","use":"sig","kid":"fixture","n":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rsa.n().to_vec()),"e":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rsa.e().to_vec())}]});
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let endpoints = Endpoints {
            issuer: ISSUER.into(),
            authorization_endpoint: AUTHORIZE.into(),
            token_endpoint: format!("{base}/token"),
            jwks_uri: format!("{base}/jwks"),
            revocation_endpoint: format!("{base}/revoke"),
        };
        let nonce = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = nonce.clone();
        let c = calls.clone();
        let revoked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rc = revoked.clone();
        let delay = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d = delay.clone();
        let seen = std::sync::Arc::new(tokio::sync::Notify::new());
        let sn = seen.clone();
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let rel = release.clone();
        let override_claims = std::sync::Arc::new(tokio::sync::Mutex::new(serde_json::json!({})));
        let oc = override_claims.clone();
        let response = std::sync::Arc::new(tokio::sync::Mutex::new(None::<(u16, String)>));
        let rsp = response.clone();
        let share = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let sh = share.clone();
        let jwks_fail = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let jf = jwks_fail.clone();
        let refresh_inputs = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let ri = refresh_inputs.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0; 16384];
                let size = stream.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..size]);
                let body = if request.starts_with("GET /jwks ") {
                    jwk.to_string()
                } else if request.starts_with("POST /revoke ") {
                    rc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    assert!(request.contains("token_type_hint=refresh_token"));
                    String::new()
                } else {
                    assert!(request.starts_with("POST /token "));
                    assert!(request.contains("client_id=oaiapp_fixture"));
                    assert!(request.contains("resource=https%3A%2F%2Fapi.openai.com%2Fv1"));
                    let form = request.split("\r\n\r\n").nth(1).unwrap_or("");
                    if let Some((_, token)) = url::form_urlencoded::parse(form.as_bytes())
                        .find(|(k, _)| k == "refresh_token")
                    {
                        ri.lock().await.push(token.into_owned());
                    }
                    let i = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let mut claims = serde_json::json!({"iss":ISSUER,"aud":"oaiapp_fixture","sub":"fixture-subject","exp":now()+3600,"nonce":n.lock().await.clone(),"email":"fixture@example.test"});
                    for (k, v) in oc.lock().await.as_object().unwrap() {
                        claims[k] = v.clone();
                    }
                    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
                    header.kid = Some("fixture".into());
                    let id_token = jsonwebtoken::encode(
                        &header,
                        &claims,
                        &jsonwebtoken::EncodingKey::from_rsa_pem(&pem).unwrap(),
                    )
                    .unwrap();
                    serde_json::json!({"access_token":format!("synthetic-access-{i}"),"refresh_token":format!("synthetic-refresh-{i}"),"id_token":id_token,"token_type":"Bearer","expires_in":3600,"scope":if sh.load(std::sync::atomic::Ordering::SeqCst){"openid email profile offline_access resource.invoke chatgpt.tokens.use.direct"}else{"openid email profile offline_access"},"earliest_refresh_at":0}).to_string()
                };
                let is_token = request.starts_with("POST /token ");
                if is_token {
                    sn.notify_one();
                    if d.load(std::sync::atomic::Ordering::SeqCst) {
                        rel.notified().await;
                    }
                }
                let (status, body) = if is_token {
                    rsp.lock().await.clone().unwrap_or((200, body))
                } else if request.starts_with("GET /jwks ")
                    && jf.swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    (503, "{}".into())
                } else {
                    (200, body)
                };
                let response=format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        Fixture {
            endpoints,
            nonce,
            calls,
            revoked,
            delay,
            seen,
            release,
            override_claims,
            response,
            share,
            jwks_fail,
            refresh_inputs,
            task,
        }
    }
    #[tokio::test]
    async fn chatgpt_exchange_verifies_identity_before_native_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        let owner = std::sync::Arc::new(value);
        let start = owner.begin(None).await.unwrap();
        let url = url::Url::parse(&start.url).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        *fixture.nonce.lock().await = params["nonce"].clone();
        let callback = format!(
            "{}?state={}&code=synthetic-code&client_id=oaiapp_fixture",
            params["redirect_uri"], params["state"]
        );
        assert_eq!(reqwest::get(callback).await.unwrap().status(), 200);
        let status = owner.status().await.unwrap();
        assert_eq!(
            status.phase, "connected",
            "only a verified inference grant can become connected"
        );
        assert_eq!(status.email.as_deref(), Some("fixture@example.test"));
        assert!(status.welcome_pending);
        let public = serde_json::to_string(&status).unwrap();
        assert!(!public.contains("synthetic-access"));
        assert!(!public.contains("synthetic-refresh"));
        assert!(!public.contains("id_token"));
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let restarted = Owner::at(dir.path().join("chatgpt"));
        assert_eq!(restarted.status().await.unwrap().phase, "connected");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.path().join("chatgpt/state.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[tokio::test]
    async fn chatgpt_attempt_rejects_wrong_state_without_consuming_listener() {
        let dir = tempfile::tempdir().unwrap();
        let owner = std::sync::Arc::new(Owner::at(dir.path().join("chatgpt")));
        let started = owner.begin(None).await;
        assert!(
            started.is_ok(),
            "native OAuth listener must start without a provider subprocess"
        );
        let started = started.unwrap();
        let url = url::Url::parse(&started.url).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["client_id"], "dynamic_agent_client");
        assert_eq!(params["code_challenge_method"], "S256");
        assert!(params["ext_agent_host_id"].starts_with("urn:uuid:"));
        assert_eq!(params["resource"], "https://api.openai.com/v1");
        let callback = &params["redirect_uri"];
        let response = reqwest::get(format!(
            "{callback}?state=wrong&code=synthetic&client_id=oaiapp_fixture"
        ))
        .await
        .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(owner.status().await.unwrap().phase, "pending");
        assert!(owner.cancel("foreign").await.is_err());
        let id = started.status.attempt_id.unwrap();
        owner.cancel(&id).await.unwrap();
        assert_eq!(owner.status().await.unwrap().phase, "disconnected");
    }
    #[tokio::test]
    async fn chatgpt_cancel_during_exchange_revokes_late_grant_without_publishing() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        let owner = std::sync::Arc::new(value);
        let start = owner.begin(None).await.unwrap();
        let p: std::collections::HashMap<_, _> = url::Url::parse(&start.url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        *fixture.nonce.lock().await = p["nonce"].clone();
        fixture
            .delay
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let url = format!(
            "{}?state={}&code=synthetic&client_id=oaiapp_fixture",
            p["redirect_uri"], p["state"]
        );
        let request = tokio::spawn(async move { reqwest::get(url).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), fixture.seen.notified())
            .await
            .unwrap();
        owner
            .cancel(&start.status.attempt_id.unwrap())
            .await
            .unwrap();
        let next = owner.begin(None).await.unwrap();
        fixture.release.notify_one();
        for _ in 0..100 {
            if fixture.revoked.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            fixture.revoked.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "cancellation must not drop an exchange that can issue a renewable grant"
        );
        assert!(owner.read().unwrap().sessions.is_empty());
        assert_eq!(
            owner.status().await.unwrap().attempt_id,
            next.status.attempt_id
        );
        owner
            .cancel(&next.status.attempt_id.unwrap())
            .await
            .unwrap();
        let _ = request.await;
    }
    #[tokio::test]
    async fn chatgpt_declined_sharing_retains_identity_for_explicit_reconsent() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        fixture
            .share
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        let owner = std::sync::Arc::new(value);
        let start = owner.begin(None).await.unwrap();
        let p: std::collections::HashMap<_, _> = url::Url::parse(&start.url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        *fixture.nonce.lock().await = p["nonce"].clone();
        reqwest::get(format!(
            "{}?state={}&code=synthetic&client_id=oaiapp_fixture",
            p["redirect_uri"], p["state"]
        ))
        .await
        .unwrap();
        assert!(
            owner
                .read()
                .unwrap()
                .sessions
                .contains_key("oaiapp_fixture"),
            "identity-only grant must retain its bound hint, never authorize inference"
        );
        assert!(owner.usable().await.is_err());
        assert_eq!(owner.status().await.unwrap().phase, "error");
        let reconnect = owner.begin(Some("oaiapp_fixture".into())).await.unwrap();
        let p: std::collections::HashMap<_, _> = url::Url::parse(&reconnect.url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(p["prompt"], "consent");
        assert!(p.contains_key("id_token_hint"));
        owner
            .cancel(&reconnect.status.attempt_id.unwrap())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn chatgpt_bad_signed_identity_never_replaces_registration() {
        for change in [
            serde_json::json!({"nonce":"wrong"}),
            serde_json::json!({"iss":"https://evil.example.test"}),
            serde_json::json!({"aud":"oaiapp_other"}),
            serde_json::json!({"exp":now()-1}),
            serde_json::json!({"sub":"different-subject"}),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let fixture = fixture().await;
            *fixture.override_claims.lock().await = change;
            let mut value = Owner::at(dir.path().join("chatgpt"));
            value.fixture_endpoints = Some(fixture.endpoints.clone());
            seeded(&value);
            let owner = std::sync::Arc::new(value);
            let start = owner.begin(Some("oaiapp_fixture".into())).await.unwrap();
            let p: std::collections::HashMap<_, _> = url::Url::parse(&start.url)
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
            *fixture.nonce.lock().await = p["nonce"].clone();
            reqwest::get(format!(
                "{}?state={}&code=synthetic",
                p["redirect_uri"], p["state"]
            ))
            .await
            .unwrap();
            assert_eq!(
                owner.read().unwrap().sessions["oaiapp_fixture"].revision,
                "old"
            );
            assert_eq!(
                owner.read().unwrap().profiles["oaiapp_fixture"].subject,
                "fixture-subject"
            );
            assert_eq!(fixture.revoked.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }
    #[tokio::test]
    async fn chatgpt_refresh_terminal_removes_only_tokens_transient_preserves_them() {
        for (status, code, removed) in [
            (400, "invalid_grant", true),
            (400, "refresh_token_reused", true),
            (503, "invalid_grant", false),
            (429, "temporarily_unavailable", false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let fixture = fixture().await;
            *fixture.response.lock().await =
                Some((status, serde_json::json!({"error":code}).to_string()));
            let mut value = Owner::at(dir.path().join("chatgpt"));
            value.fixture_endpoints = Some(fixture.endpoints.clone());
            seeded(&value);
            assert!(value.usable().await.is_err());
            let d = value.read().unwrap();
            assert_eq!(!d.sessions.contains_key("oaiapp_fixture"), removed);
            assert_eq!(d.profiles.len(), 1);
            assert!(
                d.managed_selected,
                "terminal refresh may not silently switch to CLI/API billing"
            );
        }
    }
    #[tokio::test]
    async fn chatgpt_earliest_refresh_prevents_early_rotation_and_expired_use() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&value);
        let mut d = value.read().unwrap();
        d.sessions
            .get_mut("oaiapp_fixture")
            .unwrap()
            .earliest_refresh_at = now() + 300;
        value.write(&d).unwrap();
        assert!(value.usable().await.is_err());
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        d.sessions.get_mut("oaiapp_fixture").unwrap().expires_at = now() + 30;
        value.write(&d).unwrap();
        assert_eq!(
            value.usable().await.unwrap().unwrap().access_token,
            "synthetic-old-access"
        );
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[test]
    fn chatgpt_managed_launch_pins_public_route_without_credentials_in_argv() {
        let grant = ManagedGrant {
            access_token: "synthetic-access-secret".into(),
            revision: "revision".into(),
            home: PathBuf::from("/synthetic/managed-home"),
            cache_key: "fixture".into(),
        };
        let caller = std::collections::HashMap::from([
            ("OPENAI_BASE_URL".into(), "https://evil.example.test".into()),
            ("OPENAI_API_KEY".into(), "synthetic-foreign".into()),
            ("ACCESS_TOKEN".into(), "synthetic-foreign".into()),
            ("CODEX_HOME".into(), "/foreign".into()),
            ("LD_PRELOAD".into(), "/foreign.so".into()),
            ("CODEMUX_WORKSPACE_ID".into(), "workspace".into()),
        ]);
        let (args, env) = grant.launch(caller);
        let text = args.join(" ");
        assert!(
            text.contains("https://api.openai.com/v1"),
            "managed Codex must explicitly route Responses to the public plan endpoint"
        );
        assert!(text.contains("https://api.openai.com/v1/models"));
        assert!(text.contains("supports_websockets=false"));
        assert!(text.contains("requires_openai_auth=false"));
        assert!(!text.contains("synthetic-access-secret"));
        assert_eq!(env["ACCESS_TOKEN"], "synthetic-access-secret");
        assert_eq!(env["CODEX_HOME"], "/synthetic/managed-home");
        assert_eq!(env["CODEMUX_WORKSPACE_ID"], "workspace");
        for key in ["OPENAI_BASE_URL", "OPENAI_API_KEY", "LD_PRELOAD"] {
            assert!(!env.contains_key(key));
        }
    }
    #[tokio::test]
    async fn chatgpt_cancelled_refresh_caller_cannot_abandon_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        fixture
            .delay
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&value);
        let owner = std::sync::Arc::new(value);
        let work = owner.clone();
        let caller = tokio::spawn(async move { work.usable().await });
        tokio::time::timeout(std::time::Duration::from_secs(2), fixture.seen.notified())
            .await
            .unwrap();
        caller.abort();
        let _ = caller.await;
        fixture.release.notify_one();
        for _ in 0..100 {
            if owner.read().unwrap().sessions["oaiapp_fixture"].revision != "old" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_ne!(
            owner.read().unwrap().sessions["oaiapp_fixture"].revision,
            "old",
            "a cancelled caller must not lose a rotating refresh response"
        );
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    fn seeded(owner: &Owner) {
        owner.prepare().unwrap();
        let mut disk = Disk::default();
        disk.managed_selected = true;
        disk.active_profile_id = Some("oaiapp_fixture".into());
        disk.profiles.insert(
            "oaiapp_fixture".into(),
            Registration {
                subject: "fixture-subject".into(),
                email: Some("fixture@example.test".into()),
                label: "ChatGPT account 1".into(),
                welcomed: false,
            },
        );
        disk.sessions.insert(
            "oaiapp_fixture".into(),
            Credentials {
                access_token: "synthetic-old-access".into(),
                refresh_token: "synthetic-old-refresh".into(),
                id_token: "synthetic-hint".into(),
                scopes: vec!["resource.invoke".into(), "chatgpt.tokens.use.direct".into()],
                expires_at: now() - 1,
                earliest_refresh_at: 0,
                revision: "old".into(),
            },
        );
        owner.write(&disk).unwrap();
    }
    #[tokio::test]
    async fn chatgpt_runtime_leases_block_account_changes_across_owners() {
        let dir = tempfile::tempdir().unwrap();
        let owner = std::sync::Arc::new(Owner::at(dir.path().join("chatgpt")));
        let other = Owner::at(owner.root.clone());
        let lease = other.acquire_runtime().unwrap();
        assert!(
            owner.begin(None).await.is_err(),
            "any CodeMux Codex child must fence account connect/switch"
        );
        assert!(
            owner.disconnect().await.is_err(),
            "disk-only logout cannot revoke tokens held in a child"
        );
        drop(lease);
        let start = owner.begin(None).await.unwrap();
        assert!(
            other.acquire_runtime().is_err(),
            "pending sign-in must fence new Codex launches"
        );
        owner
            .cancel(&start.status.attempt_id.unwrap())
            .await
            .unwrap();
        assert!(other.acquire_runtime().is_ok());
    }
    #[tokio::test]
    async fn chatgpt_disconnect_retains_registration_but_clears_tokens_and_welcome() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&value);
        let owner = std::sync::Arc::new(value);
        owner.usable().await.unwrap();
        let mut d = owner.read().unwrap();
        d.welcome_pending = true;
        d.host_id = "urn:uuid:fixture-host".into();
        owner.write(&d).unwrap();
        assert!(
            owner.acknowledge_welcome().await.is_ok(),
            "welcome acknowledgement must persist per registration"
        );
        assert!(
            !Owner::at(owner.root.clone())
                .status()
                .await
                .unwrap()
                .welcome_pending
        );
        let signed_out = owner.disconnect().await.unwrap();
        assert_eq!(signed_out.phase, "disconnected");
        assert!(!signed_out.welcome_pending);
        assert_eq!(signed_out.profiles.len(), 1);
        let d = owner.read().unwrap();
        assert!(d.sessions.is_empty());
        assert_eq!(d.host_id, "urn:uuid:fixture-host");
        assert!(d.profiles["oaiapp_fixture"].welcomed);
        assert!(owner.usable().await.unwrap().is_none());
        let start = owner.begin(Some("oaiapp_fixture".into())).await.unwrap();
        let params: std::collections::HashMap<_, _> = url::Url::parse(&start.url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert!(!params.contains_key("id_token_hint"));
        assert_eq!(params["client_id"], "oaiapp_fixture");
        owner
            .cancel(&start.status.attempt_id.unwrap())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn chatgpt_returning_registration_reuses_client_host_and_hints() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&value);
        let owner = std::sync::Arc::new(value);
        let start = owner.begin(Some("oaiapp_fixture".into())).await;
        assert!(
            start.is_ok(),
            "saved registration must reconnect without new dynamic client"
        );
        let start = start.unwrap();
        let url = url::Url::parse(&start.url).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["client_id"], "oaiapp_fixture");
        assert!(!params.contains_key("agent_name_hint"));
        assert_eq!(params["id_token_hint"], "synthetic-hint");
        assert_eq!(params["login_hint"], "fixture@example.test");
        assert_eq!(
            reqwest::get(format!(
                "{}?state={}&code=synthetic&client_id=oaiapp_other",
                params["redirect_uri"], params["state"]
            ))
            .await
            .unwrap()
            .status(),
            400
        );
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        *fixture.nonce.lock().await = params["nonce"].clone();
        assert_eq!(
            reqwest::get(format!(
                "{}?state={}&code=synthetic",
                params["redirect_uri"], params["state"]
            ))
            .await
            .unwrap()
            .status(),
            200
        );
        assert_eq!(owner.status().await.unwrap().phase, "connected");
        assert_eq!(owner.read().unwrap().profiles.len(), 1);
        let next = owner.begin(None).await.unwrap();
        let p: std::collections::HashMap<_, _> = url::Url::parse(&next.url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(p["ext_agent_host_id"], params["ext_agent_host_id"]);
        assert_ne!(p["state"], params["state"]);
        assert_ne!(p["nonce"], params["nonce"]);
        assert_eq!(
            owner.read().unwrap().active_profile_id.as_deref(),
            Some("oaiapp_fixture")
        );
        owner
            .cancel(&next.status.attempt_id.unwrap())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn chatgpt_refresh_rotates_once_across_two_owners() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let mut first = Owner::at(dir.path().join("chatgpt"));
        first.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&first);
        let mut second = Owner::at(first.root.clone());
        second.fixture_endpoints = Some(fixture.endpoints.clone());
        let (a, b) = tokio::join!(first.usable(), second.usable());
        assert!(
            a.is_ok(),
            "renewable managed grant must refresh: {}",
            a.err().unwrap_or_default()
        );
        let a = first.usable().await.unwrap().unwrap();
        let b = b.unwrap().unwrap();
        assert_eq!(a.access_token, b.access_token);
        assert_ne!(a.revision, "old");
        assert_eq!(
            fixture.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "rotating refresh must serialize across owners"
        );
        assert_eq!(
            first.read().unwrap().sessions["oaiapp_fixture"].refresh_token,
            "synthetic-refresh-0"
        );
    }
    #[tokio::test]
    async fn chatgpt_rotated_refresh_survives_jwks_failure_and_owner_restart() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        fixture
            .jwks_fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let root = dir.path().join("chatgpt");
        let mut value = Owner::at(root.clone());
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&value);
        assert!(
            value.usable().await.is_err(),
            "unverified replacement may not authorize inference"
        );
        let mut reopened = Owner::at(root);
        reopened.fixture_endpoints = Some(fixture.endpoints.clone());
        let grant = reopened.usable().await.unwrap().unwrap();
        assert_eq!(
            fixture.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "retry identity verification without reusing the consumed refresh token"
        );
        assert_eq!(grant.access_token, "synthetic-access-0");
    }
    #[tokio::test]
    async fn chatgpt_delayed_verification_renews_expired_candidate_without_predecessor_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let root = dir.path().join("chatgpt");
        let mut value = Owner::at(root.clone());
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&value);
        fixture
            .jwks_fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(value.usable().await.is_err());
        // Simulate delayed verification: the signed ID token remains valid,
        // but the pending access credential's original lifetime has elapsed.
        let mut disk = value.read().unwrap();
        disk.pending_refresh
            .get_mut("oaiapp_fixture")
            .unwrap()
            .credentials
            .expires_at = now() - 1;
        value.write(&disk).unwrap();
        let mut reopened = Owner::at(root);
        reopened.fixture_endpoints = Some(fixture.endpoints.clone());
        let grant = reopened.usable().await.unwrap().unwrap();
        assert_eq!(
            grant.access_token, "synthetic-access-1",
            "an expired verified candidate cannot authorize launch"
        );
        assert_eq!(
            *fixture.refresh_inputs.lock().await,
            vec!["synthetic-old-refresh", "synthetic-refresh-0"],
            "renew using the retained latest credential, never its consumed predecessor"
        );
        assert_eq!(reopened.status().await.unwrap().phase, "connected");
    }
    #[tokio::test]
    async fn chatgpt_status_restores_renewable_account_without_provider_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&value);
        let status = value.status().await.unwrap();
        assert_eq!(
            status.phase, "connected",
            "startup metadata must not lose a renewable account"
        );
        assert!(status.profiles.iter().any(|p| p.connected));
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn chatgpt_unverified_rotation_is_not_reported_ready() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture().await;
        let mut value = Owner::at(dir.path().join("chatgpt"));
        value.fixture_endpoints = Some(fixture.endpoints.clone());
        seeded(&value);
        let mut disk = value.read().unwrap();
        disk.sessions.get_mut("oaiapp_fixture").unwrap().expires_at = now() + 30;
        value.write(&disk).unwrap();
        fixture
            .jwks_fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(value.usable().await.is_err());
        fixture
            .jwks_fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let status = value.status().await.unwrap();
        assert_eq!(
            status.phase, "error",
            "metadata readiness must match the inference gate"
        );
        assert!(!status.profiles.iter().any(|p| p.connected));
    }
    #[tokio::test]
    async fn chatgpt_local_mode_survives_restart_without_cloud_auth() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Owner::at(dir.path().join("chatgpt"));
        assert!(!owner.local_workbench().await.unwrap());
        owner.set_local_workbench(true).await.unwrap();
        let restarted = Owner::at(dir.path().join("chatgpt"));
        assert!(
            restarted.local_workbench().await.unwrap(),
            "machine-local choice must survive owner recreation"
        );
        let status = serde_json::to_value(restarted.status().await.unwrap()).unwrap();
        assert_eq!(status["phase"], "disconnected");
        assert_eq!(status["activeProfileId"], serde_json::Value::Null);
        assert!(status.get("attemptId").is_some());
        assert!(status.get("welcomePending").is_some());
    }
}
