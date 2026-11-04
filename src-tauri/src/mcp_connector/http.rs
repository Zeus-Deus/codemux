//! HTTP MCP and OAuth on the connector's own loopback listener (listener.rs).
//! No paired-browser credentials.
use super::{
    authority_changed, callback_origin, changed, live, now_ms, parse_access, scope,
    validate_caller, McpConnectorState,
};
use super::{oauth, protocol};
use crate::agent_control::{self, ControlCaller, ControlError, ErrorCode};
use crate::mcp::protocol::{self as mcp_protocol, Era, RpcError};
use crate::database::{
    mcp_connector::{NewMcpClient, NewMcpGrant, NewMcpRefresh, RefreshOutcome, RefreshRotation},
    DatabaseStore,
};
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use protocol::{MAX_MCP_BODY, MAX_OAUTH_BODY};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::OwnedSemaphorePermit;

const TOKEN_TTL_MS: i64 = 3_600_000;
/// Each refresh issues a new 30-day token, so the window slides with use.
pub(crate) const REFRESH_TTL_MS: i64 = 30 * 86_400_000;
/// A spent refresh token presented again within this window fails without
/// revoking: two processes sharing one stored token may refresh together.
pub(crate) const REFRESH_REUSE_GRACE_MS: i64 = 30_000;
const GRANT_TYPES: [&str; 2] = ["authorization_code", "refresh_token"];

/// Access-token lifetime. Debug builds alone honour
/// `CODEMUX_E2E_CONNECTOR_ACCESS_TTL_SECS` (5..=3600), so live tests can
/// prove that real clients refresh.
pub(crate) fn access_ttl_ms() -> i64 {
    #[cfg(debug_assertions)]
    if let Some(secs) = std::env::var("CODEMUX_E2E_CONNECTOR_ACCESS_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|secs| (5..=3600).contains(secs))
    {
        return secs * 1000;
    }
    TOKEN_TTL_MS
}

pub fn router<R: Runtime>() -> Router<AppHandle<R>> {
    Router::new()
        .route("/mcp", any(entry::<R>))
        .route("/.well-known/oauth-protected-resource", any(entry::<R>))
        .route("/.well-known/oauth-protected-resource/mcp", any(entry::<R>))
        .route("/.well-known/oauth-authorization-server", any(entry::<R>))
        .route("/oauth/mcp/register", any(entry::<R>))
        .route("/oauth/mcp/authorize", any(entry::<R>))
        .route("/oauth/mcp/complete", any(entry::<R>))
        .route("/oauth/mcp/token", any(entry::<R>))
}

fn secure(mut response: Response) -> Response {
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    response
}
fn json_response(status: StatusCode, value: Value) -> Response {
    secure((status, Json(value)).into_response())
}
fn failure(status: StatusCode, error: &str) -> Response {
    json_response(status, json!({"error":error}))
}
fn page(status: StatusCode, html: String) -> Response {
    secure((status, axum::response::Html(html)).into_response())
}
fn consent_page(status: StatusCode, html: String, callback: &str) -> Response {
    let Ok(policy) = protocol::consent_csp(callback)
        .and_then(|p| HeaderValue::from_str(&p).map_err(|_| "Invalid callback policy".into()))
    else {
        return failure(StatusCode::BAD_REQUEST, "invalid_callback");
    };
    let mut response = page(status, html);
    response
        .headers_mut()
        .insert("content-security-policy", policy);
    // Preserve the issuer Origin on the browser's same-origin navigation POST.
    // Callback redirects still use secure()'s no-referrer policy.
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("same-origin"));
    response
}
fn method_not_allowed(allow: &'static str) -> Response {
    let mut response = failure(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allow));
    response
}
fn unauthorized(issuer: &str) -> Response {
    let mut response = failure(StatusCode::UNAUTHORIZED, "invalid_token");
    let challenge =
        format!("Bearer resource_metadata=\"{issuer}/.well-known/oauth-protected-resource/mcp\"");
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = protocol::header_value(headers, "authorization")
        .ok()
        .flatten()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || !token
            .strip_prefix(oauth::ACCESS_PREFIX)
            .is_some_and(oauth::challenge_valid)
    {
        return None;
    }
    Some(token)
}

struct Ingress {
    issuer: String,
    resource: String,
    caller: Option<ControlCaller>,
}
fn preflight<R: Runtime>(
    app: &AppHandle<R>,
    headers: &HeaderMap,
    path: &str,
) -> Result<Ingress, Response> {
    let state = app.state::<McpConnectorState>();
    let mut auth = state
        .auth
        .lock()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "connector_unavailable"))?;
    let (_, issuer, port) =
        live(app).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "connector_disabled"))?;
    auth.retain_issuer(&issuer);
    if !protocol::network_ok(headers, &issuer, port, path == "/oauth/mcp/complete") {
        return Err(failure(StatusCode::FORBIDDEN, "invalid_origin_or_host"));
    }
    let resource = format!("{issuer}/mcp");
    let caller = if path == "/mcp" {
        let token = bearer(headers).ok_or_else(|| unauthorized(&issuer))?;
        let grant = app
            .state::<DatabaseStore>()
            .mcp_grant_by_hash(&oauth::credential_hash(token), &resource, now_ms())
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "connector_unavailable"))?
            .ok_or_else(|| unauthorized(&issuer))?;
        let access = parse_access(&grant.access).map_err(|_| unauthorized(&issuer))?;
        // A token's label, else the client's self-asserted name: attribution only.
        let label = grant.label.unwrap_or(grant.client_name);
        Some(ControlCaller::outside(grant.client_id, grant.id, access).labelled(Some(label)))
    } else {
        None
    };
    Ok(Ingress {
        issuer,
        resource,
        caller,
    })
}
fn recheck<R: Runtime>(
    app: &AppHandle<R>,
    ingress: &Ingress,
    auth: &mut oauth::AuthState,
) -> Result<(), String> {
    let (_, issuer, _) = live(app)?;
    auth.retain_issuer(&issuer);
    if ingress.issuer != issuer {
        return Err("Connector origin changed".into());
    }
    Ok(())
}

async fn read_body(request: axum::http::Request<Body>, limit: usize) -> Result<Vec<u8>, Response> {
    match protocol::header_value(request.headers(), "content-length") {
        Ok(Some(value)) => match value.parse::<usize>() {
            Ok(n) if n <= limit => {}
            Ok(_) => return Err(failure(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large")),
            Err(_) => return Err(failure(StatusCode::BAD_REQUEST, "invalid_content_length")),
        },
        Ok(None) => {}
        Err(_) => return Err(failure(StatusCode::BAD_REQUEST, "invalid_content_length")),
    }
    match tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(request.into_body(), limit),
    )
    .await
    {
        Ok(Ok(bytes)) => Ok(bytes.to_vec()),
        Ok(Err(_)) => Err(failure(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large")),
        Err(_) => Err(failure(StatusCode::REQUEST_TIMEOUT, "body_timeout")),
    }
}

async fn entry<R: Runtime>(
    State(app): State<AppHandle<R>>,
    request: axum::http::Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    let permit = {
        let state = app.state::<McpConnectorState>();
        if !state
            .rate
            .lock()
            .map(|mut r| r.admit(path == "/mcp", now_ms()))
            .unwrap_or(false)
        {
            return failure(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
        }
        match state.concurrency.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => return failure(StatusCode::TOO_MANY_REQUESTS, "concurrency_limited"),
        }
    };
    let ingress = match preflight(&app, &headers, &path) {
        Ok(v) => v,
        Err(response) => return response,
    };
    let response = match path.as_str() {
        "/.well-known/oauth-protected-resource" | "/.well-known/oauth-protected-resource/mcp" => {
            if method != Method::GET {
                method_not_allowed("GET")
            } else {
                json_response(
                    StatusCode::OK,
                    json!({"resource":ingress.resource,"authorization_servers":[ingress.issuer],"scopes_supported":oauth::SCOPES,"bearer_methods_supported":["header"],"resource_name":"CodeMux native agents"}),
                )
            }
        }
        "/.well-known/oauth-authorization-server" => {
            if method != Method::GET {
                method_not_allowed("GET")
            } else {
                json_response(
                    StatusCode::OK,
                    json!({"issuer":ingress.issuer,"authorization_endpoint":format!("{}/oauth/mcp/authorize",ingress.issuer),"token_endpoint":format!("{}/oauth/mcp/token",ingress.issuer),"registration_endpoint":format!("{}/oauth/mcp/register",ingress.issuer),"response_types_supported":["code"],"grant_types_supported":GRANT_TYPES,"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"scopes_supported":oauth::SCOPES,"authorization_response_iss_parameter_supported":true}),
                )
            }
        }
        "/oauth/mcp/authorize" => {
            if method != Method::GET {
                method_not_allowed("GET")
            } else if query.len() > 8192 {
                failure(StatusCode::URI_TOO_LONG, "request_too_large")
            } else {
                authorize(&app, &ingress, &query)
            }
        }
        _ if method != Method::POST => method_not_allowed("POST"),
        "/mcp" => {
            if let Err(status) = protocol::mcp_headers(&headers, true) {
                failure(status, "invalid_mcp_headers")
            } else {
                match read_body(request, MAX_MCP_BODY).await {
                    Ok(bytes) => mcp(&app, &ingress, &headers, &bytes, permit).await,
                    Err(response) => response,
                }
            }
        }
        "/oauth/mcp/register" => {
            if !protocol::content_type(&headers, "application/json") {
                failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid_content_type")
            } else {
                match read_body(request, MAX_OAUTH_BODY).await {
                    Ok(bytes) => register(&app, &ingress, &bytes),
                    Err(r) => r,
                }
            }
        }
        "/oauth/mcp/complete" | "/oauth/mcp/token" => {
            if !protocol::content_type(&headers, "application/x-www-form-urlencoded") {
                failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid_content_type")
            } else {
                match read_body(request, MAX_OAUTH_BODY).await {
                    Ok(bytes) => {
                        if path == "/oauth/mcp/complete" {
                            complete(&app, &ingress, &headers, &bytes)
                        } else {
                            token(&app, &ingress, &headers, &bytes)
                        }
                    }
                    Err(r) => r,
                }
            }
        }
        _ => failure(StatusCode::NOT_FOUND, "not_found"),
    };
    // The request permit (unless a wait gave it up) is released here.
    response
}

fn rpc_error(id: Value, code: i64, message: &str, status: StatusCode) -> Response {
    json_response(
        status,
        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    )
}

/// The connector as a stateless-era (2026-07-28) server names itself the way
/// its `initialize` does.
const SERVER_INFO: mcp_protocol::ServerInfo =
    mcp_protocol::ServerInfo { name: "CodeMux", version: env!("CARGO_PKG_VERSION") };

/// A stateless-era rejection, `data` included (an unsupported revision lists
/// the supported ones). A method the revision lacks is 404; header, revision
/// and envelope errors are 400, which is how a dual-era client tells a
/// stateless server from an initialize-era one.
fn modern_failure(id: &Value, error: RpcError) -> Response {
    let status = if error.code == mcp_protocol::code::METHOD_NOT_FOUND {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    };
    json_response(status, json!({"jsonrpc":"2.0","id":id,"error":error}))
}

/// Which era serves this POST. Requests name it per request; notifications
/// carry no `_meta` revision, so a stateless client's ones are known by
/// their header alone. Both eras share one endpoint.
fn request_era(rpc: &protocol::Rpc, headers: &HeaderMap, id: &Value) -> Result<Era, Response> {
    let version = protocol::header_value(headers, mcp_protocol::PROTOCOL_VERSION_HEADER)
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "invalid_mcp_headers"))?;
    let era = if rpc.id.is_some() {
        Era::of_http(version, &rpc.method, &rpc.params).map_err(|error| modern_failure(id, error))?
    } else {
        Era::Legacy
    };
    match era {
        Era::Modern(_) => {
            // Duplicated or non-ASCII mirror headers are malformed.
            let mirror = |name| {
                protocol::header_value(headers, name).map_err(|_| {
                    modern_failure(id, RpcError::header_mismatch(format_args!("malformed {name} header")))
                })
            };
            let (method, name) = (mirror(mcp_protocol::METHOD_HEADER)?, mirror(mcp_protocol::NAME_HEADER)?);
            mcp_protocol::check_standard_headers(method, name, &rpc.method, &rpc.params)
                .map_err(|error| modern_failure(id, error))?;
            if !mcp_protocol::MODERN_METHODS.contains(&rpc.method.as_str()) {
                return Err(modern_failure(id, RpcError::method_not_found(&rpc.method)));
            }
        }
        Era::Legacy => {
            let unchecked = rpc.method == "initialize"
                || (rpc.id.is_none() && version.is_some_and(mcp_protocol::is_modern_version));
            protocol::mcp_headers(headers, unchecked).map_err(|status| failure(status, "invalid_mcp_headers"))?;
        }
    }
    Ok(era)
}

async fn mcp<R: Runtime>(
    app: &AppHandle<R>,
    ingress: &Ingress,
    headers: &HeaderMap,
    bytes: &[u8],
    permit: OwnedSemaphorePermit,
) -> Response {
    let value = match protocol::strict_json(bytes) {
        Ok(v) => v,
        Err(_) => return rpc_error(Value::Null, -32700, "Parse error", StatusCode::BAD_REQUEST),
    };
    let rpc = match protocol::Rpc::parse(value) {
        Ok(v) => v,
        Err((code, message)) => {
            return rpc_error(Value::Null, code, message, StatusCode::BAD_REQUEST)
        }
    };
    let id = rpc.id.clone().unwrap_or(Value::Null);
    let era = match request_era(&rpc, headers, &id) {
        Ok(era) => era,
        Err(response) => return response,
    };
    let modern = matches!(era, Era::Modern(_));
    if let Err((code, message)) = rpc.validate_method() {
        return rpc_error(
            id,
            code,
            message,
            if code == -32600 {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::OK
            },
        );
    }
    let Some(caller) = &ingress.caller else {
        return unauthorized(&ingress.issuer);
    };
    if validate_caller(app, caller).is_err() {
        return unauthorized(&ingress.issuer);
    }
    if rpc.id.is_none() {
        return secure(StatusCode::ACCEPTED.into_response());
    }
    let result = match rpc.method.as_str() {
        "server/discover" if modern => mcp_protocol::DiscoverResult {
            tools_list_changed: Some(false),
            server_info: SERVER_INFO,
            instructions: Some(agent_control::INSTRUCTIONS),
        }
        .to_value(),
        "initialize" => {
            // Echo the offer when Codemux speaks it, else the latest revision.
            let version = crate::mcp::protocol::negotiate(rpc.params["protocolVersion"].as_str());
            json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"CodeMux","version":env!("CARGO_PKG_VERSION")},"instructions":agent_control::INSTRUCTIONS})
        }
        "ping" => json!({}),
        "tools/list" => {
            let tools = json!({"tools":agent_control::registry::tools_for(caller)});
            if modern {
                // The list depends on the grant's access level.
                mcp_protocol::cacheable(tools, mcp_protocol::TOOLS_LIST_TTL_MS, mcp_protocol::CacheScope::Private)
            } else {
                tools
            }
        }
        "tools/call" => {
            // validate_method guarantees a name.
            let name = rpc.params["name"].as_str().unwrap_or_default();
            // `null` arguments mean none, as on stdio and the socket.
            let arguments = rpc.params.get("arguments").filter(|a| !a.is_null()).cloned().unwrap_or(json!({}));
            // A call that may park trades its request permit for a wait slot,
            // so parked waits never starve token refresh or other calls.
            let parks = agent_control::registry::lookup(name).is_some_and(|(_, spec)| spec.may_wait());
            let slot = if parks {
                let slot = app.state::<McpConnectorState>().admit_wait(&caller.principal);
                if slot.is_some() {
                    drop(permit);
                }
                slot.map(Some)
            } else {
                Some(None)
            };
            match slot {
                None => agent_control::registry::mcp_error(&ControlError::new(
                    ErrorCode::WaitBudgetExhausted,
                    "Too many waits in progress; retry shortly",
                )),
                // The registry gates access, validates the arguments (a bad
                // one is a tool error naming the field) and presents the
                // result. Handlers repeat grant checks before each mutation
                // and after any await. The wait slot is held until it returns.
                Some(_slot) => match agent_control::registry::call(app, caller, name, arguments).await {
                    Ok(result) => result,
                    Err(_) => return rpc_error(id, -32602, "Unknown tool", StatusCode::OK),
                },
            }
        }
        _ => return rpc_error(id, -32601, "Method not found", StatusCode::OK),
    };
    let result = if modern { mcp_protocol::complete_result(result, SERVER_INFO) } else { result };
    json_response(
        StatusCode::OK,
        json!({"jsonrpc":"2.0","id":id,"result":result}),
    )
}

fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_value(protocol::strict_json(bytes)?)
        .map_err(|_| "Invalid request fields".into())
}
fn parse_form<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_value(protocol::form_json(bytes)?).map_err(|_| "Invalid request fields".into())
}
fn register<R: Runtime>(app: &AppHandle<R>, ingress: &Ingress, bytes: &[u8]) -> Response {
    let input: oauth::Registration = match parse_json(bytes) {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_client_metadata"),
    };
    let (name, redirects) = match input.validate() {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_client_metadata"),
    };
    let id = format!("cmcp_client_{}", oauth::opaque());
    let now = now_ms();
    let result = (|| {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        recheck(app, ingress, &mut auth)?;
        let protected = auth.protected_clients(now);
        app.state::<DatabaseStore>().mcp_register_client_preserving(
            NewMcpClient {
                id: id.clone(),
                name: name.clone(),
                redirects: redirects.clone(),
                created_at: now,
            },
            now,
            &protected,
        )
    })();
    if result.is_err() {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "registration_unavailable");
    }
    json_response(
        StatusCode::CREATED,
        json!({"client_id":id,"client_id_issued_at":now/1000,"client_name":name,"redirect_uris":redirects,"token_endpoint_auth_method":"none","grant_types":GRANT_TYPES,"response_types":["code"],"scope":oauth::SCOPES.join(" ")}),
    )
}

fn waiting_html(name: &str, callback: &str, id: &str, nonce: &str) -> String {
    format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"same-origin\"><title>Connect an agent to CodeMux</title></head><body><main><h1>Connect an outside agent</h1><p>The client calls itself <strong>{}</strong>. This name is untrusted and is not proof of identity.</p><p>Access will be delivered to <strong>{}</strong>. Check this callback host before approving.</p><p>Approve or deny this request in CodeMux: open Settings → Connections → Outside assistants, or run <code>codemux connector pending</code> on that machine. Access covers <strong>all workspaces</strong> on this instance, not just the focused workspace.</p><p>Read-only is the default. Supervised clients can start and control workers but cannot approve their own worker permission requests. <strong>Full access can authorize worker commands and filesystem changes.</strong></p><p>After the owner decides in CodeMux, press Continue. This browser cannot grant or increase access.</p><form action=\"/oauth/mcp/complete\" method=\"post\"><input type=\"hidden\" name=\"request_id\" value=\"{}\"><input type=\"hidden\" name=\"nonce\" value=\"{}\"><button type=\"submit\">Continue</button></form></main></body></html>",protocol::escaped(name),protocol::escaped(callback),protocol::escaped(id),protocol::escaped(nonce))
}
fn wait_cookie(id: &str, nonce: &str, issuer: &str, clear: bool) -> Result<HeaderValue, String> {
    HeaderValue::from_str(&format!(
        "cmcp_wait_{id}={nonce}; Path=/oauth/mcp; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        if clear {
            0
        } else {
            oauth::PENDING_TTL_MS / 1000
        },
        secure = if issuer.starts_with("https://") {
            "; Secure"
        } else {
            ""
        }
    ))
    .map_err(|_| "Invalid cookie".into())
}
fn authorize<R: Runtime>(app: &AppHandle<R>, ingress: &Ingress, query: &str) -> Response {
    let input: oauth::AuthorizationInput = match parse_form(query.as_bytes()) {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    let result: Result<(String, String, String, String, String), String> = (|| {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        recheck(app, ingress, &mut auth)?;
        app.state::<DatabaseStore>()
            .mcp_prune_clients(now_ms(), &auth.protected_clients(now_ms()))?;
        let client = app
            .state::<DatabaseStore>()
            .mcp_client(&input.client_id)?
            .ok_or("Unknown registered client")?;
        let authorization =
            oauth::validate_authorization(input, client.name, &client.redirects, &ingress.issuer)?;
        let name = authorization.client_name.clone();
        let callback = callback_origin(&authorization.redirect_uri);
        protocol::consent_csp(&callback)?;
        let (id, nonce, cookie) = auth.begin(authorization, now_ms())?;
        Ok((id, nonce, cookie, name, callback))
    })();
    match result {
        Ok((id,nonce,cookie,name,callback)) => {
            let mut response = consent_page(StatusCode::OK,waiting_html(&name,&callback,&id,&nonce),&callback);
            if let Ok(cookie) = wait_cookie(&id,&cookie,&ingress.issuer,false) { response.headers_mut().insert(header::SET_COOKIE,cookie); }
            changed(app);
            response
        }
        Err(_) => page(StatusCode::BAD_REQUEST,"<!doctype html><html><head><title>Invalid connection request</title></head><body><h1>Invalid connection request</h1><p>Restart authorization from the client. The registered callback, canonical MCP resource and PKCE S256 challenge must match.</p></body></html>".into()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    request_id: String,
    nonce: String,
}
fn bound_cookie<'a>(headers: &'a HeaderMap, id: &str) -> Option<&'a str> {
    let name = format!("cmcp_wait_{id}");
    let cookies = protocol::header_value(headers, "cookie").ok().flatten()?;
    let mut matching = cookies
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .filter(|(k, _)| *k == name);
    let (_, value) = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    Some(value)
}
fn complete<R: Runtime>(
    app: &AppHandle<R>,
    ingress: &Ingress,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Response {
    let input: Completion = match parse_form(bytes) {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    if !oauth::challenge_valid(&input.request_id) {
        return failure(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let Some(cookie) = bound_cookie(headers, &input.request_id) else {
        return failure(StatusCode::FORBIDDEN, "invalid_browser_binding");
    };
    let result: Result<(Option<String>, String, String), String> = (|| {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        recheck(app, ingress, &mut auth)?;
        let callback = auth
            .pending
            .get(&input.request_id)
            .map(|p| callback_origin(&p.authorization.redirect_uri))
            .unwrap_or_default();
        let redirect = auth.complete(&input.request_id, &input.nonce, cookie, now_ms())?;
        let html = auth
            .pending
            .get(&input.request_id)
            .map(|p| {
                waiting_html(
                    &p.authorization.client_name,
                    &callback_origin(&p.authorization.redirect_uri),
                    &input.request_id,
                    &input.nonce,
                )
            })
            .unwrap_or_default();
        Ok((redirect, html, callback))
    })();
    match result {
        Ok((Some(location), _, _)) => {
            let Ok(location) = HeaderValue::from_str(&location) else {
                return failure(StatusCode::BAD_REQUEST, "invalid_redirect");
            };
            let mut response = secure(StatusCode::SEE_OTHER.into_response());
            response.headers_mut().insert(header::LOCATION, location);
            if let Ok(cookie) = wait_cookie(&input.request_id, "", &ingress.issuer, true) {
                response.headers_mut().insert(header::SET_COOKIE, cookie);
            }
            changed(app);
            response
        }
        Ok((None, html, callback)) => consent_page(StatusCode::ACCEPTED, html, &callback),
        Err(_) => failure(StatusCode::FORBIDDEN, "invalid_or_expired_browser_binding"),
    }
}
fn token<R: Runtime>(
    app: &AppHandle<R>,
    ingress: &Ingress,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Response {
    if headers.contains_key(header::AUTHORIZATION) {
        return failure(StatusCode::BAD_REQUEST, "invalid_client");
    }
    let Ok(fields) = protocol::form_json(bytes) else {
        return failure(StatusCode::BAD_REQUEST, "invalid_request");
    };
    match fields.get("grant_type").and_then(Value::as_str) {
        Some("authorization_code") => exchange_code(app, ingress, fields),
        Some("refresh_token") => refresh(app, ingress, fields),
        Some(_) => failure(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
        None => failure(StatusCode::BAD_REQUEST, "invalid_request"),
    }
}
fn exchange_code<R: Runtime>(app: &AppHandle<R>, ingress: &Ingress, fields: Value) -> Response {
    let Ok(input) = serde_json::from_value::<oauth::TokenInput>(fields) else {
        return failure(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let result: Result<Value, String> = (|| {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        recheck(app, ingress, &mut auth)?;
        let db = app.state::<DatabaseStore>();
        if db.mcp_client(&input.client_id)?.is_none() {
            return Err("Unknown client".into());
        }
        let now = now_ms();
        let ttl = access_ttl_ms();
        auth.exchange(&input, &ingress.resource, now, |code| {
            let token = oauth::access_token();
            let refresh = oauth::refresh_token();
            db.mcp_mint_grant_with_refresh(
                NewMcpGrant {
                    id: oauth::opaque(),
                    client_id: code.authorization.client_id.clone(),
                    token_hash: oauth::credential_hash(&token),
                    redirect_uri: code.authorization.redirect_uri.clone(),
                    resource: code.authorization.resource.clone(),
                    access: code.access.clone(),
                    created_at: now,
                    expires_at: now + ttl,
                },
                Some(NewMcpRefresh {
                    token_hash: oauth::credential_hash(&refresh),
                    expires_at: now + REFRESH_TTL_MS,
                }),
            )?;
            Ok(json!({"access_token":token,"token_type":"Bearer","expires_in":ttl/1000,"refresh_token":refresh,"scope":scope(&code.access)}))
        })
    })();
    match result {
        Ok(value) => {
            // The mint revoked this client's previous grant.
            authority_changed(app);
            json_response(StatusCode::OK, value)
        }
        Err(_) => failure(StatusCode::BAD_REQUEST, "invalid_grant"),
    }
}
/// `grant_type=refresh_token`: rotate the presented token. The grant id and
/// principal stay, so receipts, queued turns and in-flight work survive it.
fn refresh<R: Runtime>(app: &AppHandle<R>, ingress: &Ingress, fields: Value) -> Response {
    let Ok(input) = serde_json::from_value::<oauth::RefreshInput>(fields) else {
        return failure(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let presented = match input.validate(&ingress.resource) {
        Ok(hash) => hash,
        Err(code) => return failure(StatusCode::BAD_REQUEST, code),
    };
    let token = oauth::access_token();
    let refresh = oauth::refresh_token();
    let ttl = access_ttl_ms();
    let outcome = {
        let state = app.state::<McpConnectorState>();
        let Ok(mut auth) = state.auth.lock() else {
            return failure(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable");
        };
        // A grant is bound to the address it was issued for.
        if recheck(app, ingress, &mut auth).is_err() {
            return failure(StatusCode::BAD_REQUEST, "invalid_grant");
        }
        let now = now_ms();
        app.state::<DatabaseStore>().mcp_rotate_refresh(RefreshRotation {
            client_id: &input.client_id,
            presented_hash: &presented,
            resource: &ingress.resource,
            now,
            access_hash: oauth::credential_hash(&token),
            access_expires_at: now + ttl,
            refresh: NewMcpRefresh {
                token_hash: oauth::credential_hash(&refresh),
                expires_at: now + REFRESH_TTL_MS,
            },
            reuse_grace_ms: REFRESH_REUSE_GRACE_MS,
        })
    };
    match outcome {
        Ok(RefreshOutcome::Rotated(grant)) => {
            changed(app);
            json_response(
                StatusCode::OK,
                json!({"access_token":token,"token_type":"Bearer","expires_in":ttl/1000,"refresh_token":refresh,"scope":scope(&grant.access)}),
            )
        }
        Ok(RefreshOutcome::FamilyRevoked { client_name, .. }) => {
            log::warn!(
                "[codemux::mcp_connector] a spent refresh token for {client_name:?} came back; its sign-in was revoked"
            );
            authority_changed(app);
            failure(StatusCode::BAD_REQUEST, "invalid_grant")
        }
        Ok(RefreshOutcome::Invalid) => failure(StatusCode::BAD_REQUEST, "invalid_grant"),
        // Nothing rotated, so the client may retry with the same token.
        Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable"),
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../tests/helpers/mock_agent_provider.rs"]
mod receipt_mock;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_control::ControlAccess;
    use crate::mcp::protocol::LATEST_PROTOCOL_VERSION;
    use tauri::Manager;
    use tower::ServiceExt;

    struct Fixture {
        app: tauri::App<tauri::test::MockRuntime>,
        port: u16,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = configure(self.app.handle(), false, None, None);
        }
    }
    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }
    fn configure<R: Runtime>(
        app: &AppHandle<R>,
        enabled: bool,
        public_origin: Option<&str>,
        port: Option<u16>,
    ) -> Result<super::super::ConnectorStatus, String> {
        super::super::apply_config(
            app,
            super::super::ConfigUpdate {
                enabled,
                public_origin: public_origin.map(str::to_owned),
                port,
            },
        )
    }
    /// The connector on its own listener, on a free loopback port.
    fn listen<R: Runtime>(app: &AppHandle<R>) -> u16 {
        let port = free_port();
        let status = configure(app, true, None, Some(port)).unwrap();
        assert!(status.listener_running, "{:?}", status.bind_error);
        port
    }
    async fn fixture() -> Fixture {
        // Parent-owned native qualification only: synthetic in-memory app and
        // its explicitly loopback-only listener, never the user's instance.
        let app = tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app.manage(crate::state::AppStateStore::default());
        app.manage(crate::web_remote::WebRemoteState::default());
        app.manage(McpConnectorState::default());
        let port = listen(app.handle());
        Fixture { app, port }
    }
    fn request(
        f: &Fixture,
        path: &str,
        method: &str,
        body: &str,
        token: Option<&str>,
    ) -> axum::http::Request<Body> {
        let mut r = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("127.0.0.1:{}", f.port))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", LATEST_PROTOCOL_VERSION);
        if let Some(token) = token {
            r = r.header("authorization", format!("Bearer {token}"));
        }
        r.body(Body::from(body.to_string())).unwrap()
    }
    fn grant(f: &Fixture) -> String {
        grant_with(f, "read_only")
    }
    /// A token for the fixture client at `access`, minted without consent.
    fn grant_with(f: &Fixture, access: &str) -> String {
        let now = now_ms();
        let token = format!("cmcp_{}", oauth::opaque());
        let db = f.app.state::<DatabaseStore>();
        db.mcp_register_client(
            NewMcpClient {
                id: "fixture-client".into(),
                name: "synthetic".into(),
                redirects: vec!["https://client.example/cb".into()],
                created_at: now,
            },
            now,
        )
        .unwrap();
        db.mcp_mint_grant(NewMcpGrant {
            id: "fixture-grant".into(),
            client_id: "fixture-client".into(),
            token_hash: oauth::credential_hash(&token),
            redirect_uri: "https://client.example/cb".into(),
            resource: format!("http://127.0.0.1:{}/mcp", f.port),
            access: access.into(),
            created_at: now,
            expires_at: now + 60_000,
        })
        .unwrap();
        token
    }
    async fn invoke(f: &Fixture, request: axum::http::Request<Body>) -> Response {
        router()
            .with_state(f.app.handle().clone())
            .oneshot(request)
            .await
            .unwrap()
    }
    async fn value(response: Response) -> Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), MAX_MCP_BODY)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    fn form_request(
        f: &Fixture,
        path: &str,
        pairs: &[(&str, &str)],
        cookie: Option<&str>,
    ) -> axum::http::Request<Body> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs.iter().copied())
            .finish();
        let mut r = request(f, path, "POST", &body, None);
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded".parse().unwrap(),
        );
        r.headers_mut().insert(
            header::ORIGIN,
            format!("http://127.0.0.1:{}", f.port).parse().unwrap(),
        );
        if let Some(cookie) = cookie {
            r.headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
        }
        r
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires isolated headless Chromium/CDP driver and evidence directory"]
    async fn mario_r1_real_browser_continue_preserves_origin_and_pkce_binding() {
        let f = fixture().await;
        let origin = format!("http://127.0.0.1:{}", f.port);
        let resource = format!("{origin}/mcp");
        let evidence = std::path::PathBuf::from(std::env::var("CODEMUX_BROWSER_EVIDENCE").unwrap());
        std::fs::create_dir_all(&evidence).unwrap();
        let callback_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let callback = format!(
            "http://127.0.0.1:{}/callback",
            callback_listener.local_addr().unwrap().port()
        );
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let capture = captured.clone();
        let routes = Router::new().route("/callback", any(move |r: axum::http::Request<Body>| {
            let capture = capture.clone();
            async move {
                capture.lock().unwrap().push(json!({"uri":r.uri().to_string(),"referer":r.headers().get(header::REFERER).and_then(|h|h.to_str().ok())}));
                "Synthetic callback received"
            }
        }));
        struct AbortServer(tokio::task::JoinHandle<()>);
        impl Drop for AbortServer {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _callback_server = AbortServer(tokio::spawn(async move {
            axum::serve(callback_listener, routes).await.unwrap();
        }));
        let registration = json!({"client_name":"Synthetic browser","redirect_uris":[callback]});
        let registered = value(
            invoke(
                &f,
                request(
                    &f,
                    "/oauth/mcp/register",
                    "POST",
                    &registration.to_string(),
                    None,
                ),
            )
            .await,
        )
        .await;
        let client_id = registered["client_id"].as_str().unwrap();
        let mut authorization = url::Url::parse(&format!("{origin}/oauth/mcp/authorize")).unwrap();
        authorization.query_pairs_mut().extend_pairs([
            ("client_id", client_id),
            ("redirect_uri", callback.as_str()),
            ("response_type", "code"),
            ("resource", resource.as_str()),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
            ("state", "synthetic-browser-state"),
        ]);
        let mut child = tokio::process::Command::new("node")
            .arg(std::env::var("CODEMUX_BROWSER_DRIVER").unwrap())
            .env("CODEMUX_BROWSER_AUTHORIZE", authorization.as_str())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let binding = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(bytes) = std::fs::read(evidence.join("browser-ready.json")) {
                    break serde_json::from_slice::<Value>(&bytes).unwrap();
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        let id = binding["request_id"].as_str().unwrap();
        let nonce = binding["nonce"].as_str().unwrap();
        super::super::agent_connector_approve(
            f.app.handle().clone(),
            id.into(),
            ControlAccess::ReadOnly,
        )
        .unwrap();
        std::fs::write(evidence.join("approved"), "native owner decided read_only").unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(30), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        let result: Value =
            serde_json::from_slice(&std::fs::read(evidence.join("browser-result.json")).unwrap())
                .unwrap();
        let observed: Vec<_> = result["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["method"] == "Network.requestWillBeSentExtraInfo")
            .filter_map(|e| {
                e["params"]["headers"]
                    .get("Origin")
                    .or_else(|| e["params"]["headers"].get("origin"))
            })
            .cloned()
            .collect();
        assert!(
            result["url"].as_str().unwrap().starts_with(&callback),
            "Real browser Continue was rejected: URL={}, observed Origins={observed:?}",
            result["url"]
        );
        assert!(
            observed.contains(&json!(origin)),
            "Browser navigation POST must send the exact issuer Origin: {observed:?}"
        );
        let captured = captured.lock().unwrap().clone();
        std::fs::write(
            evidence.join("callback.json"),
            serde_json::to_vec_pretty(&captured).unwrap(),
        )
        .unwrap();
        assert_eq!(captured.len(), 1);
        assert!(
            captured[0]["referer"].is_null(),
            "Callback must not receive consent URL/code binding as Referer"
        );
        let location = url::Url::parse(result["url"].as_str().unwrap()).unwrap();
        let parameters: std::collections::HashMap<_, _> =
            location.query_pairs().into_owned().collect();
        assert_eq!(parameters["state"], "synthetic-browser-state");
        assert_eq!(parameters["iss"], origin);
        assert!(super::super::agent_connector_status(f.app.handle().clone())
            .unwrap()
            .pending
            .is_empty());
        let make = |verifier: &str| {
            form_request(
                &f,
                "/oauth/mcp/token",
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", client_id),
                    ("code", parameters["code"].as_str()),
                    ("redirect_uri", callback.as_str()),
                    ("resource", resource.as_str()),
                    ("code_verifier", verifier),
                ],
                None,
            )
        };
        assert_eq!(
            invoke(&f, make(&"x".repeat(43))).await.status(),
            StatusCode::BAD_REQUEST
        );
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(invoke(&f, make(verifier)).await.status(), StatusCode::OK);
        assert_eq!(
            invoke(&f, make(verifier)).await.status(),
            StatusCode::BAD_REQUEST
        );
        // Strict Origin rejection is unchanged, including opaque/missing origins.
        for bad in [None, Some("null"), Some("https://evil.example")] {
            let mut r = form_request(
                &f,
                "/oauth/mcp/complete",
                &[("request_id", id), ("nonce", nonce)],
                None,
            );
            r.headers_mut().remove(header::ORIGIN);
            if let Some(bad) = bad {
                r.headers_mut().insert(header::ORIGIN, bad.parse().unwrap());
            }
            assert_eq!(invoke(&f, r).await.status(), StatusCode::FORBIDDEN);
        }
    }

    async fn consent_and_exchange(f: &Fixture, client_id: &str, access: ControlAccess) -> String {
        consent_tokens(f, client_id, access).await["access_token"]
            .as_str()
            .unwrap()
            .to_string()
    }
    /// The whole token response of a fresh owner-approved sign-in.
    async fn consent_tokens(f: &Fixture, client_id: &str, access: ControlAccess) -> Value {
        let origin = format!("http://127.0.0.1:{}", f.port);
        let resource = format!("{origin}/mcp");
        let callback = "https://client.example/cb";
        let mut authorization = url::Url::parse(&format!("{origin}/oauth/mcp/authorize")).unwrap();
        authorization.query_pairs_mut().extend_pairs([
            ("client_id", client_id),
            ("redirect_uri", callback),
            ("response_type", "code"),
            ("resource", resource.as_str()),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
        ]);
        let path = format!("/oauth/mcp/authorize?{}", authorization.query().unwrap());
        let waiting = invoke(f, request(f, &path, "GET", "", None)).await;
        assert_eq!(waiting.status(),StatusCode::OK,"The cached approved registration must be usable for fresh native consent after token expiry");
        let cookie = waiting.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let id = cookie
            .split_once('=')
            .unwrap()
            .0
            .strip_prefix("cmcp_wait_")
            .unwrap();
        let html = String::from_utf8(
            axum::body::to_bytes(waiting.into_body(), MAX_OAUTH_BODY)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let nonce = html
            .split_once("name=\"nonce\" value=\"")
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap();
        // Retaining registration never grants authority or autoapproves consent.
        assert_eq!(
            invoke(
                f,
                form_request(
                    f,
                    "/oauth/mcp/complete",
                    &[("request_id", id), ("nonce", nonce)],
                    Some(&cookie)
                )
            )
            .await
            .status(),
            StatusCode::ACCEPTED
        );
        super::super::agent_connector_approve(f.app.handle().clone(), id.into(), access).unwrap();
        let redirect = invoke(
            f,
            form_request(
                f,
                "/oauth/mcp/complete",
                &[("request_id", id), ("nonce", nonce)],
                Some(&cookie),
            ),
        )
        .await;
        assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
        assert_eq!(redirect.headers()["referrer-policy"], "no-referrer");
        let location =
            url::Url::parse(redirect.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let parameters: std::collections::HashMap<_, _> =
            location.query_pairs().into_owned().collect();
        let exchanged = invoke(
            f,
            form_request(
                f,
                "/oauth/mcp/token",
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", client_id),
                    ("code", parameters["code"].as_str()),
                    ("redirect_uri", callback),
                    ("resource", resource.as_str()),
                    (
                        "code_verifier",
                        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
                    ),
                ],
                None,
            ),
        )
        .await;
        assert_eq!(exchanged.status(), StatusCode::OK);
        value(exchanged).await
    }

    #[tokio::test]
    async fn mario_r5_cached_approved_client_reauthorizes_after_one_hour_token_expiry() {
        let f = fixture().await;
        let now = now_ms();
        let old = now - TOKEN_TTL_MS - 1;
        let resource = format!("http://127.0.0.1:{}/mcp", f.port);
        let old_token = format!("cmcp_{}", oauth::opaque());
        let db = f.app.state::<DatabaseStore>();
        for id in ["cached-client", "stale-anonymous"] {
            db.mcp_register_client(
                NewMcpClient {
                    id: id.into(),
                    name: "Synthetic cached label".into(),
                    redirects: vec!["https://client.example/cb".into()],
                    created_at: old,
                },
                old,
            )
            .unwrap();
        }
        db.mcp_mint_grant(NewMcpGrant {
            id: "expired-hour-grant".into(),
            client_id: "cached-client".into(),
            token_hash: oauth::credential_hash(&old_token),
            redirect_uri: "https://client.example/cb".into(),
            resource: resource.clone(),
            access: "full_access".into(),
            created_at: old,
            expires_at: old + TOKEN_TTL_MS,
        })
        .unwrap();
        let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        assert_eq!(
            invoke(&f, request(&f, "/mcp", "POST", ping, Some(&old_token)))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let new_token = consent_and_exchange(&f, "cached-client", ControlAccess::ReadOnly).await;
        assert!(db.mcp_client("stale-anonymous").unwrap().is_none());
        assert!(db.mcp_client("cached-client").unwrap().is_some());
        assert_eq!(
            invoke(&f, request(&f, "/mcp", "POST", ping, Some(&new_token)))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(db.mcp_list_grants(now_ms()).unwrap()[0].access, "read_only");
        super::super::agent_connector_revoke(f.app.handle().clone(), "cached-client".into())
            .unwrap();
        assert_eq!(
            invoke(&f, request(&f, "/mcp", "POST", ping, Some(&new_token)))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(
            db.mcp_client("cached-client").unwrap().is_some(),
            "Revocation removes bearer authority, not the ability to request new owner consent"
        );
    }

    async fn call_tool(f: &Fixture, token: &str, name: &str, arguments: Value) -> Value {
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}});
        let response = invoke(
            f,
            request(f, "/mcp", "POST", &body.to_string(), Some(token)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        value(response).await["result"].clone()
    }
    async fn settled_receipt(f: &Fixture, token: &str, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let value =
                    call_tool(f, token, "operation_status", json!({"operation_id":id})).await;
                assert_ne!(
                    value["isError"], true,
                    "Operation receipt must belong to this authenticated client: {value}"
                );
                let receipt = value["structuredContent"].clone();
                if !matches!(receipt["state"].as_str(), Some("accepted" | "running")) {
                    break receipt;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    fn authenticated_caller(f: &Fixture, token: &str) -> ControlCaller {
        let r = request(f, "/mcp", "POST", "", Some(token));
        match preflight(f.app.handle(), r.headers(), "/mcp") {
            Ok(ingress) => ingress.caller.unwrap(),
            Err(response) => panic!("Fixture token was not authenticated: {}", response.status()),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mario_r2_reauthorization_preserves_exact_receipts_and_current_grant_authority() {
        let provider = std::sync::Arc::new(receipt_mock::MockAgentProvider::new(
            crate::agent_provider::ProviderKind::Claude,
        ));
        let (app, _root, workspace, _pane) =
            crate::agent_control::guard_tests::fixture(provider.clone(), "receipt-worker").await;
        app.manage(crate::web_remote::WebRemoteState::default());
        app.manage(McpConnectorState::default());
        let port = listen(app.handle());
        let f = Fixture { app, port };
        let _seed = grant(&f);
        let first_token =
            consent_and_exchange(&f, "fixture-client", ControlAccess::FullAccess).await;
        let first_caller = authenticated_caller(&f, &first_token);
        f.app.state::<agent_control::NativeControlState>().observe_event(&crate::agent_provider::ProviderRuntimeEvent::SessionStateChanged {
            thread_id:crate::agent_provider::ThreadId("receipt-worker".into()),
            status:crate::agent_provider::SessionStatus::Running {active_turn:crate::agent_provider::TurnId("receipt-run".into())},
        });
        let args = json!({"workspace_id":workspace,"thread_id":"receipt-worker","client_request_id":"frozen-receipt-key"});
        let first = call_tool(&f, &first_token, "thread_interrupt", args.clone()).await;
        assert_ne!(
            first["isError"], true,
            "First actual native facade operation must succeed: {first}"
        );
        let first_id = first["structuredContent"]["operation_id"].as_str().unwrap();
        let frozen = settled_receipt(&f, &first_token, first_id).await;
        assert_eq!(frozen["state"], "succeeded");
        let interruptions = || {
            provider
                .calls
                .snapshot()
                .iter()
                .filter(|c| matches!(c, receipt_mock::MockCall::InterruptTurn(..)))
                .count()
        };
        assert_eq!(interruptions(), 1);
        let replacement =
            consent_and_exchange(&f, "fixture-client", ControlAccess::FullAccess).await;
        let replacement_caller = authenticated_caller(&f, &replacement);
        assert!(
            validate_caller(f.app.handle(), &first_caller).is_err(),
            "An old caller must not borrow the new grant's authority"
        );
        assert_ne!(first_caller.grant_id, replacement_caller.grant_id);
        let retry = call_tool(&f, &replacement, "thread_interrupt", args.clone()).await;
        assert_ne!(
            retry["isError"], true,
            "Same registered client must recover its durable receipt: {retry}"
        );
        let retry_id = retry["structuredContent"]["operation_id"].as_str().unwrap();
        let recovered = settled_receipt(&f, &replacement, retry_id).await;
        if let Ok(directory) = std::env::var("CODEMUX_RECEIPT_EVIDENCE") {
            std::fs::write(
                std::path::Path::new(&directory).join("frozen-receipt.json"),
                serde_json::to_vec_pretty(&frozen).unwrap(),
            )
            .unwrap();
            std::fs::write(
                std::path::Path::new(&directory).join("recovered-receipt.json"),
                serde_json::to_vec_pretty(&recovered).unwrap(),
            )
            .unwrap();
            std::fs::write(
                std::path::Path::new(&directory).join("provider-invocations.txt"),
                interruptions().to_string(),
            )
            .unwrap();
        }
        assert_eq!(
            recovered, frozen,
            "Grant replacement must return the exact frozen receipt, never admit another operation"
        );
        assert_eq!(
            interruptions(),
            1,
            "Reauthorization must not invoke the provider twice for the same request key"
        );
        let visible = call_tool(
            &f,
            &replacement,
            "operation_status",
            json!({"operation_id":first_id}),
        )
        .await;
        assert_eq!(visible["structuredContent"], frozen);
        let mut conflicting = args.clone();
        conflicting["turn_id"] = json!("different-turn");
        let conflict = call_tool(&f, &replacement, "thread_interrupt", conflicting).await;
        assert_eq!(
            conflict["structuredContent"]["error"]["code"],
            "request_key_conflict"
        );
        let now = now_ms();
        f.app
            .state::<DatabaseStore>()
            .mcp_register_client(
                NewMcpClient {
                    id: "different-client".into(),
                    name: "synthetic".into(),
                    redirects: vec!["https://client.example/cb".into()],
                    created_at: now,
                },
                now,
            )
            .unwrap();
        let different =
            consent_and_exchange(&f, "different-client", ControlAccess::FullAccess).await;
        let not_owned = call_tool(
            &f,
            &different,
            "operation_status",
            json!({"operation_id":first_id}),
        )
        .await;
        assert_eq!(
            not_owned["structuredContent"]["error"]["code"],
            "operation_not_found"
        );
        let different_caller = authenticated_caller(&f, &different);
        let mut forged = replacement_caller.clone();
        forged.principal = different_caller.principal;
        assert!(validate_caller(f.app.handle(),&forged).is_err(),"Stable principal must match the current grant's registered owner, not a self-asserted identity");
        let mut elevated = replacement_caller.clone();
        elevated.access = ControlAccess::ReadOnly;
        assert!(validate_caller(f.app.handle(), &elevated).is_err());
        let queued = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        *f.app
            .state::<agent_control::NativeControlState>()
            .fixture_operation_barrier
            .lock()
            .unwrap() = Some(queued.clone());
        let mut held_args = args.clone();
        held_args["client_request_id"] = json!("held-old-grant");
        let held = call_tool(&f, &replacement, "thread_interrupt", held_args).await;
        let held_id = held["structuredContent"]["operation_id"].as_str().unwrap();
        let readonly = consent_and_exchange(&f, "fixture-client", ControlAccess::ReadOnly).await;
        queued.add_permits(1);
        let stale = settled_receipt(&f, &readonly, held_id).await;
        assert_eq!(stale["state"], "failed");
        assert_eq!(stale["error"]["code"], "grant_revoked");
        assert_eq!(
            interruptions(),
            1,
            "Queued work cannot adopt replacement authentication"
        );
        let read = call_tool(
            &f,
            &readonly,
            "operation_status",
            json!({"operation_id":first_id}),
        )
        .await;
        assert_eq!(read["structuredContent"], frozen);
        let denied = call_tool(&f, &readonly, "thread_interrupt", args).await;
        assert_eq!(
            denied["structuredContent"]["error"]["code"],
            "access_denied"
        );
        assert!(validate_caller(f.app.handle(), &replacement_caller).is_err());
        super::super::agent_connector_revoke(f.app.handle().clone(), "fixture-client".into())
            .unwrap();
        assert!(validate_caller(f.app.handle(), &authenticated_caller(&f, &different)).is_ok());
        let r = request(
            &f,
            "/mcp",
            "POST",
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            Some(&readonly),
        );
        assert_eq!(invoke(&f, r).await.status(), StatusCode::UNAUTHORIZED);
    }
    #[tokio::test]
    async fn registration_pressure_preserves_pending_and_code_client_bindings() {
        let f = fixture().await;
        let _token = grant(&f);
        let now = now_ms();
        let issuer = format!("http://127.0.0.1:{}", f.port);
        let ingress = Ingress {
            resource: format!("{issuer}/mcp"),
            issuer: issuer.clone(),
            caller: None,
        };
        let db = f.app.state::<DatabaseStore>();
        let mut bindings = Vec::new();
        for client_id in ["a-pending", "b-code"] {
            db.mcp_register_client(
                NewMcpClient {
                    id: client_id.into(),
                    name: "Untrusted label".into(),
                    redirects: vec!["https://client.example/cb".into()],
                    created_at: now - 1,
                },
                now,
            )
            .unwrap();
            let (id, nonce, cookie) = f
                .app
                .state::<McpConnectorState>()
                .auth
                .lock()
                .unwrap()
                .begin(
                    oauth::Authorization {
                        client_id: client_id.into(),
                        client_name: "Untrusted label".into(),
                        redirect_uri: "https://client.example/cb".into(),
                        challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into(),
                        resource: ingress.resource.clone(),
                        issuer: issuer.clone(),
                        state: None,
                    },
                    now,
                )
                .unwrap();
            super::super::agent_connector_approve(
                f.app.handle().clone(),
                id.clone(),
                ControlAccess::ReadOnly,
            )
            .unwrap();
            bindings.push((id, nonce, cookie));
        }
        let (id, nonce, cookie) = &bindings[1];
        let location = f
            .app
            .state::<McpConnectorState>()
            .auth
            .lock()
            .unwrap()
            .complete(id, nonce, cookie, now)
            .unwrap()
            .unwrap();
        for _ in 3..=crate::database::mcp_connector::MAX_CLIENTS + 1 {
            let response = register(
                f.app.handle(),
                &ingress,
                br#"{"redirect_uris":["https://client.example/cb"]}"#,
            );
            assert_eq!(response.status(), StatusCode::CREATED);
        }
        for client_id in ["a-pending", "b-code", "fixture-client"] {
            assert!(
                db.mcp_client(client_id).unwrap().is_some(),
                "Registration pressure displaced {client_id} while its authorization remained live"
            );
        }
        let url = url::Url::parse(&location).unwrap();
        let code = url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        let result = token(
            f.app.handle(),
            &ingress,
            &HeaderMap::new(),
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs([
                    ("grant_type", "authorization_code"),
                    ("client_id", "b-code"),
                    ("code", code.as_str()),
                    ("redirect_uri", "https://client.example/cb"),
                    ("resource", ingress.resource.as_str()),
                    (
                        "code_verifier",
                        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
                    ),
                ])
                .finish()
                .as_bytes(),
        );
        assert_eq!(result.status(), StatusCode::OK);
        assert_eq!(
            super::super::agent_connector_status(f.app.handle().clone())
                .unwrap()
                .pending
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn resource_rebind_invalidates_obsolete_authorization_at_every_native_boundary() {
        for boundary in ["status", "approve", "complete", "token"] {
            let f = fixture().await;
            let now = now_ms();
            let issuer = format!("http://127.0.0.1:{}", f.port);
            let resource = format!("{issuer}/mcp");
            f.app
                .state::<DatabaseStore>()
                .mcp_register_client(
                    NewMcpClient {
                        id: "bound-client".into(),
                        name: "Untrusted label".into(),
                        redirects: vec!["https://client.example/cb".into()],
                        created_at: now,
                    },
                    now,
                )
                .unwrap();
            let (id, nonce, cookie) = f
                .app
                .state::<McpConnectorState>()
                .auth
                .lock()
                .unwrap()
                .begin(
                    oauth::Authorization {
                        client_id: "bound-client".into(),
                        client_name: "Untrusted label".into(),
                        redirect_uri: "https://client.example/cb".into(),
                        challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into(),
                        resource: resource.clone(),
                        issuer,
                        state: None,
                    },
                    now,
                )
                .unwrap();
            let mut input = oauth::TokenInput {
                grant_type: "authorization_code".into(),
                client_id: "bound-client".into(),
                code: String::new(),
                redirect_uri: "https://client.example/cb".into(),
                resource: resource.clone(),
                code_verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
            };
            if boundary == "complete" || boundary == "token" {
                super::super::agent_connector_approve(
                    f.app.handle().clone(),
                    id.clone(),
                    ControlAccess::ReadOnly,
                )
                .unwrap();
            }
            if boundary == "token" {
                let location = f
                    .app
                    .state::<McpConnectorState>()
                    .auth
                    .lock()
                    .unwrap()
                    .complete(&id, &nonce, &cookie, now)
                    .unwrap()
                    .unwrap();
                input.code = url::Url::parse(&location)
                    .unwrap()
                    .query_pairs()
                    .find(|(k, _)| k == "code")
                    .unwrap()
                    .1
                    .into_owned();
            }
            // Move the address without apply_config, which would clear the
            // pending sign-ins and codes itself: each boundary must refuse
            // the obsolete resource on its own.
            let port = free_port();
            f.app
                .state::<DatabaseStore>()
                .mcp_connector_set_config(&json!({"enabled": true, "port": port}).to_string())
                .unwrap();
            super::super::listener::reconcile(f.app.handle());
            // Not through status: it would prune the obsolete requests first.
            assert_eq!(
                f.app
                    .state::<McpConnectorState>()
                    .bound_port
                    .load(std::sync::atomic::Ordering::SeqCst),
                port
            );
            let new_issuer = format!("http://127.0.0.1:{port}");
            let ingress = Ingress {
                resource: format!("{new_issuer}/mcp"),
                issuer: new_issuer,
                caller: None,
            };
            match boundary {
                "status" => assert!(
                    super::super::agent_connector_status(f.app.handle().clone())
                        .unwrap()
                        .pending
                        .is_empty(),
                    "Native status retained an obsolete resource"
                ),
                "approve" => assert!(
                    super::super::agent_connector_approve(
                        f.app.handle().clone(),
                        id.clone(),
                        ControlAccess::ReadOnly
                    )
                    .is_err(),
                    "Native approval accepted an obsolete resource"
                ),
                "complete" => {
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        header::COOKIE,
                        format!("cmcp_wait_{id}={cookie}").parse().unwrap(),
                    );
                    let form = url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs([("request_id", id.as_str()), ("nonce", nonce.as_str())])
                        .finish();
                    assert_eq!(
                        complete(f.app.handle(), &ingress, &headers, form.as_bytes()).status(),
                        StatusCode::FORBIDDEN,
                        "Browser completion minted an obsolete code"
                    );
                }
                _ => {
                    let form = url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs([
                            ("grant_type", input.grant_type.as_str()),
                            ("client_id", input.client_id.as_str()),
                            ("code", input.code.as_str()),
                            ("redirect_uri", input.redirect_uri.as_str()),
                            ("resource", input.resource.as_str()),
                            ("code_verifier", input.code_verifier.as_str()),
                        ])
                        .finish();
                    assert_eq!(
                        token(f.app.handle(), &ingress, &HeaderMap::new(), form.as_bytes())
                            .status(),
                        StatusCode::BAD_REQUEST
                    );
                    assert!(
                        f.app
                            .state::<McpConnectorState>()
                            .auth
                            .lock()
                            .unwrap()
                            .exchange(&input, &resource, now_ms(), |_| Ok(()))
                            .is_err(),
                        "An obsolete code could resurrect at the old binding"
                    );
                }
            }
            assert!(f
                .app
                .state::<McpConnectorState>()
                .auth
                .lock()
                .unwrap()
                .pending
                .is_empty());
        }
    }

    #[tokio::test]
    async fn unchanged_resources_survive_offline_denial_and_public_listener_rebind() {
        for public in [false, true] {
            let f = fixture().await;
            let issuer = if public {
                configure(f.app.handle(), true, Some("https://mcp.example"), None).unwrap();
                "https://mcp.example".to_string()
            } else {
                format!("http://127.0.0.1:{}", f.port)
            };
            let now = now_ms();
            let (id, _, _) = f
                .app
                .state::<McpConnectorState>()
                .auth
                .lock()
                .unwrap()
                .begin(
                    oauth::Authorization {
                        client_id: "synthetic-client".into(),
                        client_name: "Untrusted label".into(),
                        redirect_uri: "https://client.example/cb".into(),
                        challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into(),
                        resource: format!("{issuer}/mcp"),
                        issuer,
                        state: None,
                    },
                    now,
                )
                .unwrap();
            if public {
                // A new proxy target: the issuer is still the public origin.
                configure(f.app.handle(), true, Some("https://mcp.example"), Some(free_port()))
                    .unwrap();
            } else {
                super::super::listener::stop(f.app.handle());
            }
            assert_eq!(
                super::super::agent_connector_status(f.app.handle().clone())
                    .unwrap()
                    .pending[0]
                    .id,
                id
            );
            super::super::agent_connector_deny(f.app.handle().clone(), id).unwrap();
            let status = serde_json::to_value(
                super::super::agent_connector_status(f.app.handle().clone()).unwrap(),
            )
            .unwrap();
            assert_eq!(status["pending"][0]["phase"], "denied");
        }
    }

    #[tokio::test]
    async fn classic_cli_mode_keeps_read_grants_visible_and_natively_revocable() {
        let f = fixture().await;
        // Default state creates a CWD workspace; this read-only fixture owns
        // an explicitly empty synthetic inventory, not the checkout workspace.
        f.app.state::<crate::state::AppStateStore>().clear_workspaces();
        let observability = crate::observability::ObservabilityStore::default();
        let mut flags = observability.feature_flags();
        flags.enable_agent_chat = false;
        flags.enable_lazy_workspace_creation = false;
        // This synthetic fixture writes only the runner's disposable XDG data.
        observability.set_feature_flags(flags);
        f.app.manage(observability);
        assert!(!f
            .app
            .state::<crate::observability::ObservabilityStore>()
            .agent_chat_enabled());
        let bearer = grant(&f);
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"workspace_list","arguments":{}}}"#;
        let response = invoke(&f, request(&f, "/mcp", "POST", body, Some(&bearer))).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            value(response).await["result"]["structuredContent"],
            json!({"workspaces":[]})
        );
        assert_eq!(
            super::super::agent_connector_status(f.app.handle().clone())
                .unwrap()
                .clients
                .len(),
            1
        );
        super::super::agent_connector_revoke(f.app.handle().clone(), "fixture-client".into())
            .unwrap();
        assert!(super::super::agent_connector_status(f.app.handle().clone())
            .unwrap()
            .clients
            .is_empty());
        assert_eq!(
            invoke(&f, request(&f, "/mcp", "POST", body, Some(&bearer)))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn metadata_and_native_consent_pkce_exchange_work_without_paired_spa() {
        let f = fixture().await;
        let origin = format!("http://127.0.0.1:{}", f.port);
        let resource = format!("{origin}/mcp");
        let metadata = value(
            invoke(
                &f,
                request(
                    &f,
                    "/.well-known/oauth-authorization-server",
                    "GET",
                    "",
                    None,
                ),
            )
            .await,
        )
        .await;
        assert_eq!(metadata["issuer"], origin);
        assert_eq!(
            metadata["grant_types_supported"],
            json!(["authorization_code", "refresh_token"])
        );
        assert_eq!(
            metadata["token_endpoint_auth_methods_supported"],
            json!(["none"])
        );
        let registered = invoke(&f,request(&f,"/oauth/mcp/register","POST",r#"{"client_name":"<script>untrusted</script>","redirect_uris":["https://client.example/cb"],"token_endpoint_auth_method":"client_secret_post","grant_types":["authorization_code","refresh_token"]}"#,None)).await;
        assert_eq!(registered.status(), StatusCode::CREATED);
        let registered = value(registered).await;
        assert_eq!(registered["token_endpoint_auth_method"], "none");
        assert!(registered.get("client_secret").is_none());
        let client_id = registered["client_id"].as_str().unwrap();
        let mut authorize = url::Url::parse(&format!("{origin}/oauth/mcp/authorize")).unwrap();
        authorize.query_pairs_mut().extend_pairs([
            ("client_id", client_id),
            ("redirect_uri", "https://client.example/cb"),
            ("response_type", "code"),
            ("resource", resource.as_str()),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
            ("state", "outside-state"),
        ]);
        let path = format!("/oauth/mcp/authorize?{}", authorize.query().unwrap());
        let waiting = invoke(&f, request(&f, &path, "GET", "", None)).await;
        assert_eq!(waiting.status(), StatusCode::OK);
        assert_eq!(waiting.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(waiting.headers()["referrer-policy"], "same-origin");
        assert!(waiting.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'"));
        let cookie = waiting.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let (cookie_name, cookie_secret) = cookie.split_once('=').unwrap();
        let id = cookie_name.strip_prefix("cmcp_wait_").unwrap();
        let html = String::from_utf8(
            axum::body::to_bytes(waiting.into_body(), MAX_OAUTH_BODY)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let nonce = html
            .split_once("name=\"nonce\" value=\"")
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap();
        assert!(id != nonce && nonce != cookie_secret);
        assert!(html.contains("<meta name=\"referrer\" content=\"same-origin\">"));
        assert!(html.contains("&lt;script&gt;untrusted&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("https://client.example"));
        assert!(html.contains("all workspaces"));
        let pending = super::super::agent_connector_status(f.app.handle().clone()).unwrap();
        assert_eq!(pending.pending[0].id, id);
        assert_eq!(pending.pending[0].callback_origin, "https://client.example");
        let initial = serde_json::to_value(&pending).unwrap();
        assert_eq!(initial["pending"][0]["phase"], "awaiting_approval");
        assert!(initial["pending"][0].get("access").is_some());
        assert!(initial["pending"][0]["access"].is_null());
        let poll = invoke(
            &f,
            form_request(
                &f,
                "/oauth/mcp/complete",
                &[("request_id", id), ("nonce", nonce)],
                Some(&cookie),
            ),
        )
        .await;
        assert_eq!(poll.status(), StatusCode::ACCEPTED);
        super::super::agent_connector_approve(
            f.app.handle().clone(),
            id.into(),
            ControlAccess::ReadOnly,
        )
        .unwrap();
        let approved_status = serde_json::to_value(
            super::super::agent_connector_status(f.app.handle().clone()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            approved_status["pending"][0]["phase"],
            "approved_awaiting_client"
        );
        assert_eq!(approved_status["pending"][0]["access"], "read_only");
        assert_eq!(approved_status["clients"], json!([]));
        assert!(super::super::agent_connector_approve(
            f.app.handle().clone(),
            id.into(),
            ControlAccess::FullAccess,
        )
        .is_err());
        assert!(super::super::agent_connector_deny(f.app.handle().clone(), id.into()).is_err());
        let guessing = invoke(
            &f,
            form_request(
                &f,
                "/oauth/mcp/complete",
                &[("request_id", id), ("nonce", id)],
                Some(&cookie),
            ),
        )
        .await;
        assert_eq!(guessing.status(), StatusCode::FORBIDDEN);
        let redirect = invoke(
            &f,
            form_request(
                &f,
                "/oauth/mcp/complete",
                &[("request_id", id), ("nonce", nonce)],
                Some(&cookie),
            ),
        )
        .await;
        assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
        let continued = super::super::agent_connector_status(f.app.handle().clone()).unwrap();
        assert!(continued.pending.is_empty() && continued.clients.is_empty());
        // A completed authorization cannot be cancelled through native Deny.
        assert!(super::super::agent_connector_deny(f.app.handle().clone(), id.into()).is_err());
        let location =
            url::Url::parse(redirect.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let parameters: std::collections::HashMap<_, _> =
            location.query_pairs().into_owned().collect();
        assert_eq!(parameters["state"], "outside-state");
        assert_eq!(parameters["iss"], origin);
        let code = parameters["code"].as_str();
        let make = |verifier: &str| {
            form_request(
                &f,
                "/oauth/mcp/token",
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", client_id),
                    ("code", code),
                    ("redirect_uri", "https://client.example/cb"),
                    ("resource", resource.as_str()),
                    ("code_verifier", verifier),
                ],
                None,
            )
        };
        assert_eq!(
            invoke(&f, make(&"x".repeat(43))).await.status(),
            StatusCode::BAD_REQUEST
        );
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let token = invoke(&f, make(verifier)).await;
        assert_eq!(token.status(), StatusCode::OK);
        assert_eq!(token.headers()[header::CACHE_CONTROL], "no-store");
        let token = value(token).await;
        assert_eq!(token["scope"], "codemux:read");
        assert_eq!(token["expires_in"], 3600);
        let connected = super::super::agent_connector_status(f.app.handle().clone()).unwrap();
        assert!(connected.pending.is_empty());
        assert_eq!(connected.clients.len(), 1);
        assert_eq!(connected.clients[0].access, ControlAccess::ReadOnly);
        assert!(token["refresh_token"]
            .as_str()
            .is_some_and(|t| t.starts_with(oauth::REFRESH_PREFIX)));
        assert_eq!(
            invoke(&f, make(verifier)).await.status(),
            StatusCode::BAD_REQUEST
        );
        let bearer = token["access_token"].as_str().unwrap();
        assert_eq!(
            invoke(
                &f,
                request(
                    &f,
                    "/mcp",
                    "POST",
                    r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
                    Some(bearer)
                )
            )
            .await
            .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn mcp_grants_cannot_authenticate_broad_browser_surfaces() {
        let f = fixture().await;
        let token = grant(&f);
        let routes = crate::web_remote::server::router(f.app.handle().clone());
        for (path, method) in [
            ("/api/snapshot", "GET"),
            ("/api/assets?path=%2Fnot-a-real-user-file", "GET"),
            ("/api/ws-ticket", "POST"),
            ("/proxy/browser/1/api/health", "GET"),
        ] {
            let response = routes
                .clone()
                .oneshot(request(&f, path, method, "{}", Some(&token)))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "MCP grant reached {path}"
            );
        }
    }

    #[tokio::test]
    async fn revoked_changed_and_disabled_callers_and_concurrency_fail_closed() {
        let f = fixture().await;
        let token = grant(&f);
        let caller = ControlCaller::outside("fixture-client".into(), "fixture-grant".into(), ControlAccess::ReadOnly);
        assert!(validate_caller(f.app.handle(), &caller).is_ok());
        let elevated = ControlCaller::outside("fixture-client".into(), "fixture-grant".into(), ControlAccess::FullAccess);
        assert!(validate_caller(f.app.handle(), &elevated).is_err());
        let permit = f
            .app
            .state::<McpConnectorState>()
            .concurrency
            .clone()
            .try_acquire_many_owned(16)
            .unwrap();
        assert_eq!(
            invoke(
                &f,
                request(
                    &f,
                    "/mcp",
                    "POST",
                    r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
                    Some(&token)
                )
            )
            .await
            .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(permit);
        super::super::agent_connector_revoke(f.app.handle().clone(), "fixture-client".into())
            .unwrap();
        assert!(validate_caller(f.app.handle(), &caller).is_err());
        configure(f.app.handle(), false, None, None).unwrap();
        assert!(validate_caller(f.app.handle(), &caller).is_err());
    }

    #[tokio::test]
    async fn routes_authenticate_every_call_and_revocation_is_immediate() {
        let f = fixture().await;
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let missing = invoke(&f, request(&f, "/mcp", "POST", body, None)).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        assert!(missing.headers()[header::WWW_AUTHENTICATE]
            .to_str()
            .unwrap()
            .contains("resource_metadata="));
        let token = grant(&f);
        assert_eq!(
            invoke(&f, request(&f, "/mcp", "POST", body, Some(&token)))
                .await
                .status(),
            StatusCode::OK
        );
        super::super::agent_connector_revoke(f.app.handle().clone(), "fixture-client".into())
            .unwrap();
        assert_eq!(
            invoke(&f, request(&f, "/mcp", "POST", body, Some(&token)))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    #[tokio::test]
    async fn mario_r6_initialize_negotiates_valid_versions_but_headers_stay_strict() {
        let f = fixture().await;
        let token = grant(&f);
        let initialize = |version: Value| {
            json!({"jsonrpc":"2.0","id":"version-negotiation","method":"initialize","params":{
                "protocolVersion":version,"capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}
            }})
            .to_string()
        };
        // A supported offer is echoed; any other version string negotiates
        // the latest revision instead of failing.
        for (offer, answer) in [
            ("2024-11-05", "2024-11-05"),
            ("2025-03-26", "2025-03-26"),
            ("2025-06-18", "2025-06-18"),
            ("2025-11-25", "2025-11-25"),
            ("2030-01-01", LATEST_PROTOCOL_VERSION),
            ("2026-07-28", LATEST_PROTOCOL_VERSION),
            ("wrong", LATEST_PROTOCOL_VERSION),
            ("2025-02-30", LATEST_PROTOCOL_VERSION),
            ("2025-6-18", LATEST_PROTOCOL_VERSION),
        ] {
            let mut r = request(&f, "/mcp", "POST", &initialize(json!(offer)), Some(&token));
            r.headers_mut().remove("mcp-protocol-version");
            let response = invoke(&f, r).await;
            assert_eq!(response.status(), StatusCode::OK);
            let response = value(response).await;
            assert_eq!(response["result"]["protocolVersion"], answer, "{offer}: {response}");
            assert!(response.get("error").is_none());
        }
        for invalid in [Value::Null, json!(1), json!(""), json!("x".repeat(4096))] {
            let response = value(
                invoke(&f, request(&f, "/mcp", "POST", &initialize(invalid), Some(&token))).await,
            )
            .await;
            assert_eq!(response["error"]["code"], -32602);
        }
        let missing = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}}"#;
        assert_eq!(
            value(invoke(&f, request(&f, "/mcp", "POST", missing, Some(&token))).await).await
                ["error"]["code"],
            -32602
        );
        let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        for (version, status) in [
            (None, StatusCode::OK),
            (Some("2024-11-05"), StatusCode::OK),
            (Some("null"), StatusCode::BAD_REQUEST),
        ] {
            let mut r = request(&f, "/mcp", "POST", ping, Some(&token));
            r.headers_mut().remove("mcp-protocol-version");
            if let Some(version) = version {
                r.headers_mut()
                    .insert("mcp-protocol-version", version.parse().unwrap());
            }
            assert_eq!(invoke(&f, r).await.status(), status, "{version:?}");
        }
        assert_eq!(
            invoke(&f, request(&f, "/mcp", "POST", ping, Some(&token)))
                .await
                .status(),
            StatusCode::OK
        );
    }
    #[tokio::test]
    async fn exact_stateless_protocol_statuses_and_read_only_authority() {
        let f = fixture().await;
        let token = grant(&f);
        for method in ["GET", "DELETE"] {
            assert_eq!(
                invoke(&f, request(&f, "/mcp", method, "", Some(&token)))
                    .await
                    .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
        }
        let init = r#"{"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}"#;
        let response = invoke(&f, request(&f, "/mcp", "POST", init, Some(&token))).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key("mcp-session-id"));
        let init = value(response).await;
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            init["result"]["capabilities"],
            json!({"tools":{"listChanged":false}})
        );
        let list = value(
            invoke(
                &f,
                request(
                    &f,
                    "/mcp",
                    "POST",
                    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                    Some(&token),
                ),
            )
            .await,
        )
        .await;
        let listed: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            listed.len(),
            crate::agent_control::registry::tools_for(&authenticated_caller(&f, &token)).len()
        );
        assert!(
            !listed.contains(&"thread_stop") && listed.contains(&"thread_read"),
            "a read-only grant is offered only what it can run: {listed:?}"
        );
        assert!(list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| !["terminal_send", "git_status", "browser_click"]
                .contains(&t["name"].as_str().unwrap())));
        let n = invoke(
            &f,
            request(
                &f,
                "/mcp",
                "POST",
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                Some(&token),
            ),
        )
        .await;
        assert_eq!(n.status(), StatusCode::ACCEPTED);
        assert!(axum::body::to_bytes(n.into_body(), 100)
            .await
            .unwrap()
            .is_empty());
        let write = value(invoke(&f,request(&f,"/mcp","POST",r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"thread_stop","arguments":{"workspace_id":"missing","thread_id":"missing","client_request_id":"request"}}}"#,Some(&token))).await).await;
        assert_eq!(write["result"]["isError"], true);
        assert_eq!(
            write["result"]["structuredContent"]["error"]["code"],
            "access_denied"
        );
        configure(f.app.handle(), false, None, None).unwrap();
        assert_eq!(
            invoke(
                &f,
                request(&f, "/mcp", "POST", init.to_string().as_str(), Some(&token))
            )
            .await
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    #[tokio::test]
    async fn invalid_tool_arguments_are_tool_errors_with_detail() {
        let f = fixture().await;
        let token = grant(&f);
        let result = call_tool(&f, &token, "thread_list", json!({"bogus": 1})).await;
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(result["content"][0]["text"], "invalid_arguments: unknown field `bogus`");
        assert_eq!(
            result["structuredContent"]["error"],
            json!({"code": "invalid_arguments", "message": "unknown field `bogus`", "retryable": false})
        );
    }
    #[tokio::test]
    async fn unknown_tools_stay_json_rpc_errors() {
        let f = fixture().await;
        let token = grant(&f);
        // A legacy stdio tool and an in-app tool are both unknown here.
        for name in ["terminal_write", "create_threads", "nope"] {
            let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":{}}});
            let response = invoke(&f, request(&f, "/mcp", "POST", &body.to_string(), Some(&token))).await;
            assert_eq!(response.status(), StatusCode::OK);
            let response = value(response).await;
            assert_eq!(response["error"]["code"], -32602, "{name}: {response}");
            assert!(response.get("result").is_none());
        }
    }
    #[tokio::test]
    async fn plugin_invoke_input_reaches_the_plugin_layer() {
        let f = fixture().await;
        let token = grant_with(&f, "full_access");
        let result = call_tool(&f, &token, "plugin_invoke", json!({
            "installationId": "missing", "generation": "g", "actionId": "a", "operationId": "op", "input": {"a": 1},
        }))
        .await;
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["error"]["code"], "plugin_operation_failed",
            "an untyped input passes the schema and reaches the plugin layer: {result}"
        );
    }
    #[tokio::test]
    async fn tool_errors_carry_retryable() {
        let f = fixture().await;
        f.app.manage(agent_control::NativeControlState::default());
        let token = grant(&f);
        let result = call_tool(&f, &token, "operation_status", json!({"operation_id": "missing"})).await;
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(
            result["content"][0]["text"],
            "operation_not_found: No operation belongs to this client and ID"
        );
        assert_eq!(
            result["structuredContent"]["error"],
            json!({"code": "operation_not_found", "message": "No operation belongs to this client and ID", "retryable": false})
        );
    }
    #[tokio::test]
    async fn csrf_spoofed_hosts_body_limits_and_ambiguous_envelopes_fail_closed() {
        let f = fixture().await;
        let token = grant(&f);
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut forged = request(&f, "/mcp", "POST", body, Some(&token));
        forged
            .headers_mut()
            .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert_eq!(invoke(&f, forged).await.status(), StatusCode::FORBIDDEN);
        let oversized = "x".repeat(MAX_MCP_BODY + 1);
        assert_eq!(
            invoke(&f, request(&f, "/mcp", "POST", &oversized, Some(&token)))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        for body in ["[]", r#"{"jsonrpc":"2.0","id":1,"id":2,"method":"ping"}"#] {
            assert_eq!(
                invoke(&f, request(&f, "/mcp", "POST", body, Some(&token)))
                    .await
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
    }

    // ── Refresh tokens, personal tokens, the own listener ────────────────

    fn refresh_request(f: &Fixture, client_id: &str, refresh: &str) -> axum::http::Request<Body> {
        form_request(
            f,
            "/oauth/mcp/token",
            &[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("refresh_token", refresh),
            ],
            None,
        )
    }
    async fn refreshed(f: &Fixture, client_id: &str, refresh: &str) -> (StatusCode, Value) {
        let response = invoke(f, refresh_request(f, client_id, refresh)).await;
        (response.status(), value(response).await)
    }
    async fn ping(f: &Fixture, token: &str) -> StatusCode {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        invoke(f, request(f, "/mcp", "POST", body, Some(token)))
            .await
            .status()
    }
    /// The connector plus a chat (`thread`) that thread tools can target. Wait
    /// signals are process-wide and tagged by thread id, so each test names its
    /// own worker and never wakes another test's waits.
    async fn worker_fixture(thread: &str) -> (
        Fixture,
        std::sync::Arc<receipt_mock::MockAgentProvider>,
        tempfile::TempDir,
        String,
    ) {
        let provider = std::sync::Arc::new(receipt_mock::MockAgentProvider::new(
            crate::agent_provider::ProviderKind::Claude,
        ));
        let (app, root, workspace, _pane) =
            crate::agent_control::guard_tests::fixture(provider.clone(), thread).await;
        app.manage(crate::web_remote::WebRemoteState::default());
        app.manage(McpConnectorState::default());
        let port = listen(app.handle());
        (Fixture { app, port }, provider, root, workspace)
    }
    fn worker_runs(f: &Fixture, thread: &str) {
        f.app.state::<agent_control::NativeControlState>().observe_event(
            &crate::agent_provider::ProviderRuntimeEvent::SessionStateChanged {
                thread_id: crate::agent_provider::ThreadId(thread.into()),
                status: crate::agent_provider::SessionStatus::Running {
                    active_turn: crate::agent_provider::TurnId("busy-run".into()),
                },
            },
        );
    }
    fn issue(f: &Fixture, label: &str, access: ControlAccess) -> super::super::IssuedToken {
        super::super::agent_connector_issue(f.app.handle().clone(), label.into(), access, None)
            .unwrap()
    }
    /// A `thread_wait` on the busy worker, sent from its own task.
    fn spawn_wait(f: &Fixture, token: &str, workspace: &str, thread: &str) -> tokio::task::JoinHandle<Value> {
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"thread_wait","arguments":{
            "workspace_id":workspace,"thread_id":thread,"timeout_ms":30_000
        }}});
        let r = request(f, "/mcp", "POST", &body.to_string(), Some(token));
        let app = f.app.handle().clone();
        tokio::spawn(async move {
            let response = router().with_state(app).oneshot(r).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            value(response).await["result"].clone()
        })
    }
    fn free_wait_slots(f: &Fixture) -> usize {
        f.app.state::<McpConnectorState>().waiters.available_permits()
    }
    /// Waits until `parked` waits hold slots, and checks they stay parked.
    async fn parked(f: &Fixture, parked: usize, waits: &[tokio::task::JoinHandle<Value>]) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while free_wait_slots(f) != super::super::MAX_WAITS - parked {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("expected {parked} parked waits, {} slots free", free_wait_slots(f)));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(waits.iter().all(|w| !w.is_finished()), "a wait returned instead of parking");
    }
    fn error_code(result: &Value) -> &str {
        result["structuredContent"]["error"]["code"].as_str().unwrap_or_default()
    }
    /// Every wait slot comes back within a second.
    async fn slots_return(f: &Fixture, after: &str) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while free_wait_slots(f) != super::super::MAX_WAITS {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{after}: {} slots free", free_wait_slots(f)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refresh_grant_rotates_and_keeps_receipts_and_in_flight_authority() {
        let worker = "refresh-worker";
        let (f, provider, _root, workspace) = worker_fixture(worker).await;
        let _seed = grant(&f);
        let first = consent_tokens(&f, "fixture-client", ControlAccess::FullAccess).await;
        let (a1, r1) = (first["access_token"].as_str().unwrap(), first["refresh_token"].as_str().unwrap());
        assert!(r1.starts_with(oauth::REFRESH_PREFIX));
        let caller = authenticated_caller(&f, a1);
        assert_eq!(caller.label.as_deref(), Some("synthetic"), "the client's name attributes its work");
        worker_runs(&f, worker);
        let args = json!({"workspace_id":workspace,"thread_id":worker,"client_request_id":"before-refresh"});
        let receipt = call_tool(&f, a1, "thread_interrupt", args.clone()).await;
        assert_ne!(receipt["isError"], true, "{receipt}");
        let id = receipt["structuredContent"]["operation_id"].as_str().unwrap().to_string();
        let frozen = settled_receipt(&f, a1, &id).await;
        let (status, second) = refreshed(&f, "fixture-client", r1).await;
        assert_eq!(status, StatusCode::OK, "{second}");
        let (a2, r2) = (second["access_token"].as_str().unwrap(), second["refresh_token"].as_str().unwrap());
        assert_ne!(a2, a1);
        assert_ne!(r2, r1);
        assert_eq!(second["token_type"], "Bearer");
        assert_eq!(second["expires_in"], 3600);
        assert_eq!(second["scope"], "codemux:read codemux:full");
        assert_eq!(ping(&f, a1).await, StatusCode::UNAUTHORIZED, "the old access token stops working");
        assert_eq!(ping(&f, a2).await, StatusCode::OK);
        let refreshed_caller = authenticated_caller(&f, a2);
        assert_eq!(refreshed_caller.grant_id, caller.grant_id, "a refresh keeps the grant");
        assert_eq!(refreshed_caller.principal, caller.principal);
        assert!(validate_caller(f.app.handle(), &caller).is_ok(), "work admitted before the refresh keeps its authority");
        let visible = call_tool(&f, a2, "operation_status", json!({"operation_id":id})).await;
        assert_eq!(visible["structuredContent"], frozen);
        // A retry after the refresh replays the receipt instead of acting twice.
        let retry = call_tool(&f, a2, "thread_interrupt", args).await;
        assert_eq!(retry["structuredContent"]["operation_id"], id.as_str(), "{retry}");
        let interrupts = provider
            .calls
            .snapshot()
            .iter()
            .filter(|c| matches!(c, receipt_mock::MockCall::InterruptTurn(..)))
            .count();
        assert_eq!(interrupts, 1);
        let status = super::super::agent_connector_status(f.app.handle().clone()).unwrap();
        let client = &status.clients[0];
        assert!(client.renews && client.kind == super::super::ClientKind::Oauth);
        let lapses = chrono::DateTime::parse_from_rfc3339(&client.expires_at).unwrap().timestamp_millis();
        assert!(lapses > now_ms() + REFRESH_TTL_MS - 60_000, "a renewing sign-in lapses with its refresh token");
    }

    #[tokio::test]
    async fn refresh_reuse_is_answered_with_invalid_grant_and_a_revoked_family_needs_new_consent() {
        let f = fixture().await;
        let _seed = grant(&f);
        let first = consent_tokens(&f, "fixture-client", ControlAccess::ReadOnly).await;
        let r1 = first["refresh_token"].as_str().unwrap();
        let (status, second) = refreshed(&f, "fixture-client", r1).await;
        assert_eq!(status, StatusCode::OK);
        let r2 = second["refresh_token"].as_str().unwrap();
        // Inside the grace window a spent token fails without harm.
        let (status, reuse) = refreshed(&f, "fixture-client", r1).await;
        assert_eq!((status, reuse), (StatusCode::BAD_REQUEST, json!({"error":"invalid_grant"})));
        let (status, third) = refreshed(&f, "fixture-client", r2).await;
        assert_eq!(status, StatusCode::OK, "the latest token still rotates");
        let (a3, r3) = (third["access_token"].as_str().unwrap(), third["refresh_token"].as_str().unwrap());
        assert_eq!(ping(&f, a3).await, StatusCode::OK);
        // After the grace window, a spent token revokes the whole family.
        let resource = format!("http://127.0.0.1:{}/mcp", f.port);
        let now = now_ms();
        let outcome = f
            .app
            .state::<DatabaseStore>()
            .mcp_rotate_refresh(RefreshRotation {
                client_id: "fixture-client",
                presented_hash: &oauth::credential_hash(r2),
                resource: &resource,
                now: now + REFRESH_REUSE_GRACE_MS + 1,
                access_hash: oauth::credential_hash(&oauth::access_token()),
                access_expires_at: now + TOKEN_TTL_MS,
                refresh: NewMcpRefresh {
                    token_hash: oauth::credential_hash(&oauth::refresh_token()),
                    expires_at: now + REFRESH_TTL_MS,
                },
                reuse_grace_ms: REFRESH_REUSE_GRACE_MS,
            })
            .unwrap();
        assert!(matches!(outcome, RefreshOutcome::FamilyRevoked { .. }));
        assert_eq!(ping(&f, a3).await, StatusCode::UNAUTHORIZED);
        let (status, _) = refreshed(&f, "fixture-client", r3).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a revoked family needs a new owner approval");
        assert!(super::super::agent_connector_status(f.app.handle().clone()).unwrap().clients.is_empty());
    }

    #[tokio::test]
    async fn token_endpoint_rejects_mixed_unknown_and_header_authenticated_refresh_requests() {
        let f = fixture().await;
        let _seed = grant(&f);
        let tokens = consent_tokens(&f, "fixture-client", ControlAccess::ReadOnly).await;
        let (access, refresh) = (tokens["access_token"].as_str().unwrap(), tokens["refresh_token"].as_str().unwrap());
        let resource = format!("http://127.0.0.1:{}/mcp", f.port);
        let base = [("grant_type", "refresh_token"), ("client_id", "fixture-client"), ("refresh_token", refresh)];
        let with = |extra: &[(&'static str, &'static str)]| -> Vec<(&str, &str)> {
            base.iter().copied().chain(extra.iter().copied()).collect()
        };
        let cases: Vec<(Vec<(&str, &str)>, &str)> = vec![
            (with(&[("code", "x"), ("code_verifier", "y")]), "invalid_request"),
            (with(&[("redirect_uri", "https://client.example/cb")]), "invalid_request"),
            (vec![("grant_type", "password"), ("client_id", "fixture-client")], "unsupported_grant_type"),
            (vec![("client_id", "fixture-client"), ("refresh_token", refresh)], "invalid_request"),
            (vec![("grant_type", "refresh_token"), ("client_id", "fixture-client"), ("refresh_token", access)], "invalid_grant"),
            (with(&[("resource", "https://other.example/mcp")]), "invalid_grant"),
            (with(&[("scope", "codemux:admin")]), "invalid_scope"),
            (vec![("grant_type", "refresh_token"), ("client_id", "other-client"), ("refresh_token", refresh)], "invalid_grant"),
        ];
        for (pairs, error) in cases {
            let response = invoke(&f, form_request(&f, "/oauth/mcp/token", &pairs, None)).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{pairs:?}");
            assert_eq!(value(response).await, json!({"error":error}), "{pairs:?}");
        }
        let mut header = refresh_request(&f, "fixture-client", refresh);
        header
            .headers_mut()
            .insert(header::AUTHORIZATION, "Basic Zm9vOmJhcg==".parse().unwrap());
        let response = invoke(&f, header).await;
        assert_eq!(value(response).await, json!({"error":"invalid_client"}));
        // A refresh token is never a bearer.
        assert_eq!(ping(&f, refresh).await, StatusCode::UNAUTHORIZED);
        // None of the refused requests spent the token.
        let (status, ok) = refreshed(&f, "fixture-client", refresh).await;
        assert_eq!(status, StatusCode::OK, "{ok}");
        let explicit = [
            ("grant_type", "refresh_token"),
            ("client_id", "fixture-client"),
            ("refresh_token", ok["refresh_token"].as_str().unwrap()),
            ("resource", resource.as_str()),
            ("scope", "codemux:read"),
        ];
        let response = invoke(&f, form_request(&f, "/oauth/mcp/token", &explicit, None)).await;
        assert_eq!(response.status(), StatusCode::OK, "the canonical resource and a known scope are accepted");
    }

    #[tokio::test]
    async fn metadata_and_registration_advertise_the_refresh_grant() {
        let f = fixture().await;
        let metadata = value(invoke(&f, request(&f, "/.well-known/oauth-authorization-server", "GET", "", None)).await).await;
        assert_eq!(metadata["grant_types_supported"], json!(["authorization_code", "refresh_token"]));
        let registered = invoke(
            &f,
            request(&f, "/oauth/mcp/register", "POST", r#"{"client_name":"Refreshing client","redirect_uris":["https://client.example/cb"]}"#, None),
        )
        .await;
        assert_eq!(registered.status(), StatusCode::CREATED);
        assert_eq!(value(registered).await["grant_types"], json!(["authorization_code", "refresh_token"]));
    }

    #[tokio::test]
    async fn personal_token_authenticates_mcp_under_its_label_principal() {
        let f = fixture().await;
        let issued = issue(&f, "hermes-bot", ControlAccess::Supervised);
        assert_eq!(issued.mcp_url, format!("http://127.0.0.1:{}/mcp", f.port));
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"hermes","version":"1"}}}"#;
        let response = invoke(&f, request(&f, "/mcp", "POST", init, Some(&issued.token))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let list = value(invoke(&f, request(&f, "/mcp", "POST", r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, Some(&issued.token))).await).await;
        let names: Vec<_> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"thread_launch"), "{names:?}");
        let workspaces = call_tool(&f, &issued.token, "workspace_list", json!({})).await;
        assert_ne!(workspaces["isError"], true, "{workspaces}");
        let caller = authenticated_caller(&f, &issued.token);
        assert_eq!(caller.principal, format!("mcp-client:{}", issued.client_id));
        assert_eq!(caller.label.as_deref(), Some("hermes-bot"));
        assert_eq!(caller.access, ControlAccess::Supervised);
        let reader = issue(&f, "reader", ControlAccess::ReadOnly);
        let stop = call_tool(&f, &reader.token, "thread_stop", json!({"workspace_id":"missing","thread_id":"missing","client_request_id":"r"})).await;
        assert_eq!(error_code(&stop), "access_denied");
        super::super::agent_connector_revoke(f.app.handle().clone(), issued.client_id.clone()).unwrap();
        assert_eq!(ping(&f, &issued.token).await, StatusCode::UNAUTHORIZED);
        assert_eq!(ping(&f, &reader.token).await, StatusCode::OK, "revoking one label leaves the others");
    }

    #[tokio::test]
    async fn connector_routes_are_not_served_by_the_web_remote_router() {
        let f = fixture().await;
        let token = grant(&f);
        let routes = crate::web_remote::server::router(f.app.handle().clone());
        let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let token_form = "grant_type=authorization_code&client_id=fixture-client";
        for (path, method, body) in [
            ("/mcp", "POST", ping),
            ("/.well-known/oauth-authorization-server", "GET", ""),
            ("/.well-known/oauth-protected-resource/mcp", "GET", ""),
            ("/oauth/mcp/register", "POST", r#"{"redirect_uris":["https://client.example/cb"]}"#),
            ("/oauth/mcp/token", "POST", token_form),
        ] {
            let response = routes.clone().oneshot(request(&f, path, method, body, Some(&token))).await.unwrap();
            assert!(!response.headers().contains_key(header::WWW_AUTHENTICATE), "{path}");
            let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024).await.unwrap();
            let text = String::from_utf8_lossy(&bytes);
            for leaked in ["\"issuer\"", "access_token", "\"jsonrpc\"", "client_id"] {
                assert!(!text.contains(leaked), "web remote served {path}: {leaked}");
            }
        }
    }

    #[tokio::test]
    async fn waiting_page_names_desktop_and_cli_approval() {
        let f = fixture().await;
        let _seed = grant(&f);
        let origin = format!("http://127.0.0.1:{}", f.port);
        let mut authorize = url::Url::parse(&format!("{origin}/oauth/mcp/authorize")).unwrap();
        authorize.query_pairs_mut().extend_pairs([
            ("client_id", "fixture-client"),
            ("redirect_uri", "https://client.example/cb"),
            ("response_type", "code"),
            ("resource", format!("{origin}/mcp").as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
        ]);
        let path = format!("/oauth/mcp/authorize?{}", authorize.query().unwrap());
        let waiting = invoke(&f, request(&f, &path, "GET", "", None)).await;
        assert_eq!(waiting.status(), StatusCode::OK);
        let html = String::from_utf8(axum::body::to_bytes(waiting.into_body(), MAX_OAUTH_BODY).await.unwrap().to_vec()).unwrap();
        assert!(html.contains("Settings → Connections → Outside assistants"), "{html}");
        assert!(html.contains("<code>codemux connector pending</code>"), "{html}");
        assert!(!html.contains("Outside agents") && !html.contains("Settings → MCP"), "stale path: {html}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn parked_waits_use_the_waiter_budget_not_the_general_permits() {
        let worker = "budget-worker";
        let (f, _provider, _root, workspace) = worker_fixture(worker).await;
        let _seed = grant(&f);
        let oauth_tokens = consent_tokens(&f, "fixture-client", ControlAccess::ReadOnly).await;
        // With every wait slot taken a wait is refused at once, as a
        // retryable tool error, and other calls are unaffected.
        let held = f.app.state::<McpConnectorState>().waiters.clone().try_acquire_many_owned(super::super::MAX_WAITS as u32).unwrap();
        let bot = issue(&f, "wait-bot-a", ControlAccess::ReadOnly);
        let refused = spawn_wait(&f, &bot.token, &workspace, worker).await.unwrap();
        assert_eq!(refused["isError"], true, "{refused}");
        assert_eq!(refused["structuredContent"]["error"], json!({"code":"wait_budget_exhausted","message":"Too many waits in progress; retry shortly","retryable":true}));
        assert_eq!(ping(&f, &bot.token).await, StatusCode::OK);
        drop(held);
        // As many parked waits as there are request permits leave room for
        // everything else, token refresh included.
        worker_runs(&f, worker);
        let other = issue(&f, "wait-bot-b", ControlAccess::ReadOnly);
        let waits: Vec<_> = (0..super::super::MAX_WAITS_PER_CLIENT * 2)
            .map(|i| spawn_wait(&f, if i % 2 == 0 { &bot.token } else { &other.token }, &workspace, worker))
            .collect();
        parked(&f, waits.len(), &waits).await;
        assert_eq!(f.app.state::<McpConnectorState>().concurrency.available_permits(), 16, "parked waits hold no request permit");
        let status = call_tool(&f, &bot.token, "thread_status", json!({"workspace_id":workspace,"thread_id":worker})).await;
        assert_ne!(status["isError"], true, "{status}");
        let (refresh_status, _) = refreshed(&f, "fixture-client", oauth_tokens["refresh_token"].as_str().unwrap()).await;
        assert_eq!(refresh_status, StatusCode::OK, "token refresh still gets a permit");
        super::super::agent_connector_revoke_all(f.app.handle().clone()).unwrap();
        for wait in waits {
            assert_eq!(error_code(&wait.await.unwrap()), "grant_revoked");
        }
        assert_eq!(free_wait_slots(&f), super::super::MAX_WAITS, "every slot is returned");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn waits_are_capped_per_client() {
        let worker = "capped-worker";
        let (f, _provider, _root, workspace) = worker_fixture(worker).await;
        worker_runs(&f, worker);
        let greedy = issue(&f, "greedy-bot", ControlAccess::ReadOnly);
        let waits: Vec<_> = (0..super::super::MAX_WAITS_PER_CLIENT)
            .map(|_| spawn_wait(&f, &greedy.token, &workspace, worker))
            .collect();
        parked(&f, waits.len(), &waits).await;
        let ninth = spawn_wait(&f, &greedy.token, &workspace, worker).await.unwrap();
        assert_eq!(error_code(&ninth), "wait_budget_exhausted", "{ninth}");
        let polite = issue(&f, "polite-bot", ControlAccess::ReadOnly);
        let theirs = spawn_wait(&f, &polite.token, &workspace, worker);
        parked(&f, waits.len() + 1, std::slice::from_ref(&theirs)).await;
        super::super::agent_connector_revoke_all(f.app.handle().clone()).unwrap();
        for wait in waits.into_iter().chain([theirs]) {
            assert_eq!(error_code(&wait.await.unwrap()), "grant_revoked");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revocation_ends_a_parked_wait() {
        let worker = "revoked-worker";
        let (f, _provider, _root, workspace) = worker_fixture(worker).await;
        worker_runs(&f, worker);
        let bot = issue(&f, "revoked-bot", ControlAccess::ReadOnly);
        let wait = spawn_wait(&f, &bot.token, &workspace, worker);
        parked(&f, 1, std::slice::from_ref(&wait)).await;
        let started = std::time::Instant::now();
        super::super::agent_connector_revoke(f.app.handle().clone(), bot.client_id.clone()).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(2), wait).await.expect("the wait outlived its grant").unwrap();
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        assert_eq!(error_code(&ended), "grant_revoked", "{ended}");
        assert_eq!(free_wait_slots(&f), super::super::MAX_WAITS);
        // Disabling the connector ends waits too.
        let bot = issue(&f, "paused-bot", ControlAccess::ReadOnly);
        let wait = spawn_wait(&f, &bot.token, &workspace, worker);
        parked(&f, 1, std::slice::from_ref(&wait)).await;
        configure(f.app.handle(), false, None, None).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(2), wait).await.expect("the wait outlived the switch").unwrap();
        assert_eq!(error_code(&ended), "connector_disabled", "{ended}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abandoned_wait_frees_its_slot() {
        let worker = "abandoned-worker";
        let (f, _provider, _root, workspace) = worker_fixture(worker).await;
        worker_runs(&f, worker);
        let bot = issue(&f, "impatient-bot", ControlAccess::ReadOnly);
        // The request future is dropped part-way.
        let wait = spawn_wait(&f, &bot.token, &workspace, worker);
        parked(&f, 1, std::slice::from_ref(&wait)).await;
        wait.abort();
        slots_return(&f, "an aborted request").await;
        // A client whose HTTP timeout fires hangs up on the real listener.
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"thread_wait","arguments":{
            "workspace_id":workspace,"thread_id":worker,"timeout_ms":30_000
        }}})
        .to_string();
        let raw = format!(
            "POST /mcp HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\ncontent-type: application/json\r\n\
             accept: application/json, text/event-stream\r\nmcp-protocol-version: {LATEST_PROTOCOL_VERSION}\r\n\
             authorization: Bearer {token}\r\ncontent-length: {length}\r\n\r\n{body}",
            port = f.port,
            token = bot.token,
            length = body.len(),
        );
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", f.port)).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut client, raw.as_bytes()).await.unwrap();
        parked(&f, 1, &[]).await;
        drop(client);
        slots_return(&f, "a client that hung up").await;
        // The same client is not locked out: it gets its whole share again.
        let waits: Vec<_> = (0..super::super::MAX_WAITS_PER_CLIENT)
            .map(|_| spawn_wait(&f, &bot.token, &workspace, worker))
            .collect();
        parked(&f, waits.len(), &waits).await;
        super::super::agent_connector_revoke_all(f.app.handle().clone()).unwrap();
        for wait in waits {
            assert_eq!(error_code(&wait.await.unwrap()), "grant_revoked");
        }
    }

    // ── Protocol versions and envelopes ──────────────────────────────────

    #[tokio::test]
    async fn connector_accepts_initialize_with_a_protocol_header() {
        let f = fixture().await;
        let token = grant(&f);
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"hermes","version":"1"}}}"#;
        for header in ["2025-11-25", "2026-07-28"] {
            let mut r = request(&f, "/mcp", "POST", body, Some(&token));
            r.headers_mut().insert("mcp-protocol-version", header.parse().unwrap());
            let response = invoke(&f, r).await;
            assert_eq!(response.status(), StatusCode::OK, "{header}");
            assert_eq!(value(response).await["result"]["protocolVersion"], "2025-11-25");
        }
    }

    #[tokio::test]
    async fn connector_accepts_2025_11_25_client_info_fields() {
        let f = fixture().await;
        let token = grant(&f);
        let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"elicitation":{}},"clientInfo":{
            "name":"claude-code","version":"2.1.0","title":"Claude Code","description":"An outside assistant",
            "websiteUrl":"https://client.example","icons":[{"src":"https://client.example/icon.png","mimeType":"image/png"}]
        }}});
        let response = value(invoke(&f, request(&f, "/mcp", "POST", &body.to_string(), Some(&token))).await).await;
        assert_eq!(response["result"]["protocolVersion"], "2025-11-25", "{response}");
        assert_eq!(response["result"]["serverInfo"]["name"], "CodeMux");
    }

    /// A stateless-era (2026-07-28) POST the way dual-era SDKs send it: the
    /// `_meta` envelope in the body, mirrored by the version, method and
    /// name headers.
    fn modern_request(f: &Fixture, id: u64, method: &str, mut params: Value, token: Option<&str>) -> axum::http::Request<Body> {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "hermes", "version": "2.0.0"},
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        let name = params["name"].as_str().map(str::to_owned);
        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let mut r = request(f, "/mcp", "POST", &body.to_string(), token);
        let headers = r.headers_mut();
        headers.insert("mcp-protocol-version", "2026-07-28".parse().unwrap());
        headers.insert("mcp-method", method.parse().unwrap());
        if let Some(name) = name {
            headers.insert("mcp-name", name.parse().unwrap());
        }
        r
    }

    fn server_info() -> Value {
        json!({"name": "CodeMux", "version": env!("CARGO_PKG_VERSION")})
    }

    /// The `stateless_*` tests check stateless serving. With the kill switch
    /// off every request is initialize-era, which the other tests cover.
    fn stateless_era_off() -> bool {
        if !mcp_protocol::MODERN_ERA_ENABLED {
            eprintln!("skipped: the stateless era is switched off (mcp::protocol::MODERN_ERA_ENABLED)");
        }
        !mcp_protocol::MODERN_ERA_ENABLED
    }

    #[tokio::test]
    async fn stateless_clients_discover_list_and_call_beside_initialize_era_ones() {
        if stateless_era_off() {
            return;
        }
        let f = fixture().await;
        let token = grant(&f);
        let response = invoke(&f, modern_request(&f, 1, "server/discover", json!({}), Some(&token))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let discover = value(response).await;
        assert_eq!(discover["id"], 1);
        let result = &discover["result"];
        assert_eq!(result["resultType"], "complete", "{discover}");
        assert_eq!(result["supportedVersions"], json!(["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"]));
        assert_eq!(result["capabilities"], json!({"tools": {"listChanged": false}}));
        assert_eq!(result["instructions"], agent_control::INSTRUCTIONS);
        assert_eq!(result["_meta"]["io.modelcontextprotocol/serverInfo"], server_info());
        assert_eq!((result["ttlMs"].as_u64(), result["cacheScope"].as_str()), (Some(3_600_000), Some("public")));

        let modern_list = value(invoke(&f, modern_request(&f, 2, "tools/list", json!({}), Some(&token))).await).await;
        let legacy_list = value(invoke(&f, request(&f, "/mcp", "POST", r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#, Some(&token))).await).await;
        assert_eq!(modern_list["result"]["tools"], legacy_list["result"]["tools"], "one catalog per grant for both eras");
        assert!(legacy_list["result"].get("resultType").is_none(), "initialize-era results are unchanged");
        let listed = &modern_list["result"];
        assert_eq!(listed["resultType"], "complete");
        assert_eq!((listed["ttlMs"].as_u64(), listed["cacheScope"].as_str()), (Some(300_000), Some("private")));
        assert_eq!(listed["_meta"]["io.modelcontextprotocol/serverInfo"], server_info());

        let call = value(invoke(&f, modern_request(&f, 4, "tools/call", json!({"name": "workspace_list", "arguments": {}}), Some(&token))).await).await;
        assert_eq!(call["result"]["isError"], false, "{call}");
        assert_eq!(call["result"]["resultType"], "complete");
        assert!(call["result"]["structuredContent"]["workspaces"].is_array(), "{call}");
        // Tool failures stay tool results; a read grant is refused writes.
        let refused = value(invoke(&f, modern_request(&f, 5, "tools/call", json!({"name": "thread_stop", "arguments": {
            "workspace_id": "missing", "thread_id": "missing", "client_request_id": "request"
        }}), Some(&token))).await).await;
        assert_eq!(refused["result"]["structuredContent"]["error"]["code"], "access_denied", "{refused}");
        assert_eq!(refused["result"]["resultType"], "complete");

        // Credentials are still checked first-class on the stateless path.
        assert_eq!(invoke(&f, modern_request(&f, 6, "tools/list", json!({}), None)).await.status(), StatusCode::UNAUTHORIZED);
        // A stateless client's notification carries its revision only in the header.
        let mut cancelled = request(&f, "/mcp", "POST", r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":4}}"#, Some(&token));
        cancelled.headers_mut().insert("mcp-protocol-version", "2026-07-28".parse().unwrap());
        assert_eq!(invoke(&f, cancelled).await.status(), StatusCode::ACCEPTED);
    }

    /// The status and JSON-RPC error of one rejected POST.
    async fn rejected(f: &Fixture, r: axum::http::Request<Body>) -> (StatusCode, Value) {
        let response = invoke(f, r).await;
        let status = response.status();
        (status, value(response).await["error"].clone())
    }

    #[tokio::test]
    async fn stateless_headers_must_mirror_the_body() {
        if stateless_era_off() {
            return;
        }
        let f = fixture().await;
        let token = grant(&f);
        let call = || modern_request(&f, 7, "tools/call", json!({"name": "thread_list", "arguments": {}}), Some(&token));
        let cases: Vec<(&str, axum::http::Request<Body>)> = vec![
            ("missing Mcp-Method", { let mut r = call(); r.headers_mut().remove("mcp-method"); r }),
            ("other Mcp-Method", { let mut r = call(); r.headers_mut().insert("mcp-method", "tools/list".parse().unwrap()); r }),
            ("missing Mcp-Name", { let mut r = call(); r.headers_mut().remove("mcp-name"); r }),
            ("other Mcp-Name", { let mut r = call(); r.headers_mut().insert("mcp-name", "thread_stop".parse().unwrap()); r }),
            ("two Mcp-Name", { let mut r = call(); r.headers_mut().append("mcp-name", "thread_list".parse().unwrap()); r }),
            ("missing version", { let mut r = call(); r.headers_mut().remove("mcp-protocol-version"); r }),
            ("other version", { let mut r = call(); r.headers_mut().insert("mcp-protocol-version", "2025-11-25".parse().unwrap()); r }),
        ];
        for (case, r) in cases {
            let (status, error) = rejected(&f, r).await;
            assert_eq!((status, error["code"].clone()), (StatusCode::BAD_REQUEST, json!(-32020)), "{case}: {error}");
        }
        // A Base64-encoded name is decoded before it is compared.
        let mut encoded = call();
        encoded.headers_mut().insert("mcp-name", "=?base64?dGhyZWFkX2xpc3Q=?=".parse().unwrap());
        assert_eq!(invoke(&f, encoded).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stateless_revision_errors_are_modern_json_rpc_errors() {
        if stateless_era_off() {
            return;
        }
        let f = fixture().await;
        let token = grant(&f);
        // A revision Codemux does not speak lists the ones it does, so a
        // dual-era client retries instead of falling back.
        let body = json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{
            "io.modelcontextprotocol/protocolVersion": "2099-01-01",
            "io.modelcontextprotocol/clientCapabilities": {}
        }}});
        let mut future = request(&f, "/mcp", "POST", &body.to_string(), Some(&token));
        future.headers_mut().insert("mcp-protocol-version", "2099-01-01".parse().unwrap());
        future.headers_mut().insert("mcp-method", "server/discover".parse().unwrap());
        let (status, error) = rejected(&f, future).await;
        assert_eq!((status, error["code"].clone()), (StatusCode::BAD_REQUEST, json!(-32022)), "{error}");
        assert_eq!(error["data"]["supported"][0], "2026-07-28");
        assert_eq!(error["data"]["requested"], "2099-01-01");
        // A stateless header on a body that lost its envelope is malformed.
        let mut bare = request(&f, "/mcp", "POST", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#, Some(&token));
        bare.headers_mut().insert("mcp-protocol-version", "2026-07-28".parse().unwrap());
        let (status, error) = rejected(&f, bare).await;
        assert_eq!((status, error["code"].clone()), (StatusCode::BAD_REQUEST, json!(-32602)), "{error}");
        // `ping` is not part of the stateless revision.
        let (status, error) = rejected(&f, modern_request(&f, 2, "ping", json!({}), Some(&token))).await;
        assert_eq!((status, error["code"].clone()), (StatusCode::NOT_FOUND, json!(-32601)), "{error}");
        // Without an envelope, discover is method-not-found: the fallback signal.
        let legacy = r#"{"jsonrpc":"2.0","id":3,"method":"server/discover","params":{}}"#;
        let response = value(invoke(&f, request(&f, "/mcp", "POST", legacy, Some(&token))).await).await;
        assert_eq!((response["id"].clone(), response["error"]["code"].clone()), (json!(3), json!(-32601)));
    }

    #[tokio::test]
    async fn null_arguments_read_as_no_arguments() {
        let f = fixture().await;
        let token = grant(&f);
        let result = call_tool(&f, &token, "workspace_list", Value::Null).await;
        assert_eq!(result["isError"], false, "{result}");
        assert_eq!(result, call_tool(&f, &token, "workspace_list", json!({})).await);
    }

    #[tokio::test]
    async fn connector_acks_unknown_notifications() {
        let f = fixture().await;
        let token = grant(&f);
        for body in [
            r#"{"jsonrpc":"2.0","method":"notifications/roots/list_changed"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"p","progress":1}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#,
        ] {
            let response = invoke(&f, request(&f, "/mcp", "POST", body, Some(&token))).await;
            assert_eq!(response.status(), StatusCode::ACCEPTED, "{body}");
            assert!(axum::body::to_bytes(response.into_body(), 100).await.unwrap().is_empty());
        }
        // A request method without an id is still not a notification.
        let response = invoke(&f, request(&f, "/mcp", "POST", r#"{"jsonrpc":"2.0","method":"tools/list"}"#, Some(&token))).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn tools_list_refuses_a_cursor_it_never_issued() {
        let f = fixture().await;
        let token = grant(&f);
        let body = r#"{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{"cursor":"page-2"}}"#;
        let response = value(invoke(&f, request(&f, "/mcp", "POST", body, Some(&token))).await).await;
        assert_eq!(response["id"], 4);
        assert_eq!(response["error"], json!({"code":-32602,"message":"Invalid cursor"}));
    }
}
