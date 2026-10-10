//! HTTP MCP on the existing Axum listener. No paired-browser credentials.
use super::{
    callback_origin, changed, live, now_ms, parse_access, scope, validate_caller, McpConnectorState,
};
use super::{oauth, protocol};
use crate::agent_control::{self, ControlAccess, ControlCaller};
use crate::database::{
    mcp_connector::{NewMcpClient, NewMcpGrant},
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
use protocol::{MAX_MCP_BODY, MAX_OAUTH_BODY, PROTOCOL};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

const TOKEN_TTL_MS: i64 = 3_600_000;

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
            .strip_prefix("cmcp_")
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
        Some(ControlCaller::outside(
            grant.id,
            parse_access(&grant.access).map_err(|_| unauthorized(&issuer))?,
        ))
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
                    json!({"issuer":ingress.issuer,"authorization_endpoint":format!("{}/oauth/mcp/authorize",ingress.issuer),"token_endpoint":format!("{}/oauth/mcp/token",ingress.issuer),"registration_endpoint":format!("{}/oauth/mcp/register",ingress.issuer),"response_types_supported":["code"],"grant_types_supported":["authorization_code"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"scopes_supported":oauth::SCOPES,"authorization_response_iss_parameter_supported":true}),
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
                    Ok(bytes) => mcp(&app, &ingress, &headers, &bytes).await,
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
    drop(permit);
    response
}

fn rpc_error(id: Value, code: i64, message: &str, status: StatusCode) -> Response {
    json_response(
        status,
        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    )
}
fn tool_error(code: &str, message: &str) -> Value {
    json!({"isError":true,"content":[{"type":"text","text":message}],"structuredContent":{"error":{"code":code,"message":message}}})
}
async fn mcp<R: Runtime>(
    app: &AppHandle<R>,
    ingress: &Ingress,
    headers: &HeaderMap,
    bytes: &[u8],
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
    if let Err(status) = protocol::mcp_headers(headers, rpc.method == "initialize") {
        return failure(status, "invalid_mcp_headers");
    }
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
        "initialize" => {
            json!({"protocolVersion":PROTOCOL,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"CodeMux","version":env!("CARGO_PKG_VERSION")},"instructions":agent_control::INSTRUCTIONS})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":agent_control::remote_tools()}),
        "tools/call" => {
            let name = rpc.params["name"].as_str().unwrap();
            let arguments = rpc.params.get("arguments").cloned().unwrap_or(json!({}));
            let tools = agent_control::remote_tools();
            let Some(tool) = tools.iter().find(|t| t.name == name) else {
                return rpc_error(id, -32602, "Unknown tool", StatusCode::OK);
            };
            if !protocol::schema_valid(&tool.input_schema, &arguments) {
                return rpc_error(id, -32602, "Invalid tool arguments", StatusCode::OK);
            }
            if caller.access == ControlAccess::ReadOnly && tool.annotations["readOnlyHint"] != true
            {
                tool_error(
                    "access_denied",
                    "Read-only connections cannot modify native threads",
                )
            } else {
                // The native owner repeats grant/access checks immediately
                // before each mutation and after any admission await.
                match agent_control::execute(app, caller, name, arguments).await {
                    Ok(value) => {
                        json!({"isError":false,"content":[{"type":"text","text":value.to_string()}],"structuredContent":if value.is_object() { value } else { json!({"result":value}) }})
                    }
                    Err(error) => tool_error(error.code, &error.message),
                }
            }
        }
        _ => return rpc_error(id, -32601, "Method not found", StatusCode::OK),
    };
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
        json!({"client_id":id,"client_id_issued_at":now/1000,"client_name":name,"redirect_uris":redirects,"token_endpoint_auth_method":"none","grant_types":["authorization_code"],"response_types":["code"],"scope":oauth::SCOPES.join(" ")}),
    )
}

fn waiting_html(name: &str, callback: &str, id: &str, nonce: &str) -> String {
    format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>Connect an agent to CodeMux</title></head><body><main><h1>Connect an outside agent</h1><p>The client calls itself <strong>{}</strong>. This name is untrusted and is not proof of identity.</p><p>Access will be delivered to <strong>{}</strong>. Check this callback host before approving.</p><p>Open CodeMux Settings → MCP → Outside agents and approve or deny this request. Access covers <strong>all workspaces</strong> on this instance, not just the focused workspace.</p><p>Read-only is the default. Supervised clients can start and control workers but cannot approve their own worker permission requests. <strong>Full access can authorize worker commands and filesystem changes.</strong></p><p>After the owner decides in CodeMux, press Continue. This browser cannot grant or increase access.</p><form action=\"/oauth/mcp/complete\" method=\"post\"><input type=\"hidden\" name=\"request_id\" value=\"{}\"><input type=\"hidden\" name=\"nonce\" value=\"{}\"><button type=\"submit\">Continue</button></form></main></body></html>",protocol::escaped(name),protocol::escaped(callback),protocol::escaped(id),protocol::escaped(nonce))
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
    let input: oauth::TokenInput = match parse_form(bytes) {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    if input.grant_type != "authorization_code" {
        return failure(StatusCode::BAD_REQUEST, "unsupported_grant_type");
    }
    let result: Result<Value, String> = (|| {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        recheck(app, ingress, &mut auth)?;
        let db = app.state::<DatabaseStore>();
        if db.mcp_client(&input.client_id)?.is_none() {
            return Err("Unknown client".into());
        }
        let now = now_ms();
        auth.exchange(&input,&ingress.resource,now,|code| {
            let token = format!("cmcp_{}",oauth::opaque());
            db.mcp_mint_grant(NewMcpGrant { id:oauth::opaque(), client_id:code.authorization.client_id.clone(), token_hash:oauth::credential_hash(&token), redirect_uri:code.authorization.redirect_uri.clone(), resource:code.authorization.resource.clone(), access:code.access.clone(), created_at:now, expires_at:now+TOKEN_TTL_MS })?;
            Ok(json!({"access_token":token,"token_type":"Bearer","expires_in":TOKEN_TTL_MS/1000,"scope":scope(&code.access)}))
        })
    })();
    match result {
        Ok(value) => {
            changed(app);
            json_response(StatusCode::OK, value)
        }
        Err(_) => failure(StatusCode::BAD_REQUEST, "invalid_grant"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Manager;
    use tower::ServiceExt;

    struct Fixture {
        app: tauri::App<tauri::test::MockRuntime>,
        port: u16,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = crate::web_remote::web_remote_disable(self.app.handle().clone());
        }
    }
    async fn fixture() -> Fixture {
        // Parent-owned native qualification only: synthetic in-memory app and
        // its explicitly loopback-only listener, never the user's instance.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let app = tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app.manage(crate::state::AppStateStore::default());
        app.manage(crate::web_remote::WebRemoteState::default());
        app.manage(McpConnectorState::default());
        crate::web_remote::web_remote_set_config(
            app.handle().clone(),
            Some(port),
            None,
            Some("loopback".into()),
            Some(false),
            Some(false),
            Some(false),
            Some(true),
        )
        .await
        .unwrap();
        crate::web_remote::web_remote_enable(app.handle().clone())
            .await
            .unwrap();
        super::super::agent_connector_set_config(app.handle().clone(), true, None).unwrap();
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
            .header("mcp-protocol-version", PROTOCOL);
        if let Some(token) = token {
            r = r.header("authorization", format!("Bearer {token}"));
        }
        r.body(Body::from(body.to_string())).unwrap()
    }
    fn grant(f: &Fixture) -> String {
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
            access: "read_only".into(),
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
            let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = reservation.local_addr().unwrap().port();
            drop(reservation);
            crate::web_remote::web_remote_set_config(
                f.app.handle().clone(),
                Some(port),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
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
                super::super::agent_connector_set_config(
                    f.app.handle().clone(),
                    true,
                    Some("https://mcp.example".into()),
                )
                .unwrap();
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
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let port = listener.local_addr().unwrap().port();
                drop(listener);
                crate::web_remote::web_remote_set_config(
                    f.app.handle().clone(),
                    Some(port),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .unwrap();
            } else {
                crate::web_remote::web_remote_disable(f.app.handle().clone()).unwrap();
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
            json!(["authorization_code"])
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
        assert!(token.get("refresh_token").is_none());
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
        let caller = ControlCaller::outside("fixture-grant".into(), ControlAccess::ReadOnly);
        assert!(validate_caller(f.app.handle(), &caller).is_ok());
        let elevated = ControlCaller::outside("fixture-grant".into(), ControlAccess::FullAccess);
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
        super::super::agent_connector_set_config(f.app.handle().clone(), false, None).unwrap();
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
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL);
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
        assert_eq!(
            list["result"]["tools"].as_array().unwrap().len(),
            crate::agent_control::remote_tools().len()
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
        super::super::agent_connector_set_config(f.app.handle().clone(), false, None).unwrap();
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
}
