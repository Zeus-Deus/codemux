use super::{ErrorCode, ProtocolError, Result};
use codemux_addon_protocol::manifest::{HttpGrant, HttpMethod};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub context: String,
    pub origin: String,
    pub path: String,
    pub method: HttpMethod,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}
#[derive(Default)]
struct Traffic {
    entries: VecDeque<(Instant, usize)>,
    total: usize,
}
#[derive(Clone)]
pub struct Http {
    slots: Arc<Semaphore>,
    traffic: Arc<Mutex<Traffic>>,
    #[cfg(test)]
    fixture: Arc<Mutex<Option<VecDeque<Response>>>>,
    #[cfg(test)]
    observed: Arc<Mutex<Vec<serde_json::Value>>>,
}
impl Default for Http {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(4)),
            traffic: Arc::new(Mutex::new(Traffic::default())),
            #[cfg(test)]
            fixture: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            observed: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
fn denied() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::NetworkDenied,
        "This network request is not allowed",
    )
}
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && (b == 168 || (b == 0 && (c == 0 || c == 2))))
        && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}
pub fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] <= 0x01ff || s[1] == 0x0db8))
                && s[0] != 0x2002
        }
    }
}
pub fn validate(request: &Request, grant: &HttpGrant) -> Result<url::Url> {
    if request.origin != grant.origin
        || !grant.methods.contains(&request.method)
        || !request.path.starts_with('/')
        || request.path.starts_with("//")
        || request.path.contains('\\')
        || request.path.chars().any(char::is_control)
        || request.body.as_ref().is_some_and(|s| s.len() > 256 * 1024)
        || request.headers.len() > 20
    {
        return Err(denied());
    }
    let url =
        url::Url::parse(&format!("{}{}", request.origin, request.path)).map_err(|_| denied())?;
    if url.origin().ascii_serialization() != grant.origin
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(denied());
    }
    for (key, value) in &request.headers {
        let lower = key.to_ascii_lowercase();
        if ![
            "accept",
            "content-type",
            "if-none-match",
            "if-modified-since",
        ]
        .contains(&lower.as_str())
            || key.len() > 64
            || value.len() > 4096
            || value.chars().any(char::is_control)
        {
            return Err(denied());
        }
    }
    Ok(url)
}
impl Http {
    // Compiled out of production; only the test harness can replace transport.
    // The ordinary manager, grant, context, request and quota checks still run.
    #[cfg(test)]
    pub(super) async fn recorded_responses(&self, responses: Vec<Response>) {
        *self.fixture.lock().await = Some(responses.into());
    }
    #[cfg(test)]
    pub(super) async fn observed_requests(&self) -> Vec<serde_json::Value> {
        self.observed.lock().await.clone()
    }
    async fn charge(&self, size: usize) -> Result<()> {
        let mut traffic = self.traffic.lock().await;
        let now = Instant::now();
        while traffic
            .entries
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) >= Duration::from_secs(61))
        {
            let (_, n) = traffic.entries.pop_front().unwrap();
            traffic.total -= n
        }
        if traffic.total + size > 10 * 1024 * 1024 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Network traffic limit",
            ));
        }
        traffic.total += size;
        // One bucket per second bounds accounting memory even for byte-sized
        // chunks. The bucket timestamp is its end, conservatively retaining
        // traffic up to one second beyond the rolling window.
        if let Some((time, bytes)) = traffic
            .entries
            .back_mut()
            .filter(|(time, _)| now.duration_since(*time) < Duration::from_secs(1))
        {
            *bytes += size;
            let _ = time;
        } else {
            traffic.entries.push_back((now, size));
        }
        Ok(())
    }
    pub async fn fetch(
        &self,
        plugin: &str,
        request: Request,
        grant: &HttpGrant,
        credential: Option<String>,
        cancel: &CancellationToken,
    ) -> Result<Response> {
        let _slot = self.slots.try_acquire().map_err(|_| {
            ProtocolError::new(ErrorCode::ResourceLimit, "Too many network requests")
        })?;
        let url = validate(&request, grant)?;
        let host = url.host_str().ok_or_else(denied)?.to_string();
        let work = async {
            #[cfg(test)]
            {
                let fixture = self
                    .fixture
                    .lock()
                    .await
                    .as_mut()
                    .map(|responses| responses.pop_front());
                if let Some(response) = fixture {
                    let mut response = response.ok_or_else(denied)?;
                    self.observed.lock().await.push(serde_json::json!({"origin":request.origin,"path":request.path,"method":request.method,"body":request.body,"credentialAttached":credential.is_some()}));
                    self.charge(request.body.as_ref().map_or(0, String::len))
                        .await?;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    if response.body.len() > 512 * 1024 {
                        return Err(ProtocolError::new(
                            ErrorCode::ResourceLimit,
                            "HTTP response exceeds 512 KiB",
                        ));
                    }
                    self.charge(response.body.len()).await?;
                    if (300..400).contains(&response.status) {
                        return Err(denied());
                    }
                    response.headers.retain(|key, value| {
                        [
                            "content-type",
                            "etag",
                            "retry-after",
                            "x-ratelimit-limit",
                            "x-ratelimit-remaining",
                            "x-ratelimit-reset",
                        ]
                        .contains(&key.as_str())
                            && value.len() <= 4096
                    });
                    return Ok(response);
                }
            }
            let addresses = tokio::net::lookup_host((host.as_str(), 443))
                .await
                .map_err(|_| denied())?
                .collect::<Vec<_>>();
            if addresses.is_empty() || addresses.iter().any(|a| !public_address(a.ip())) {
                return Err(denied());
            }
            // DNS is resolved once, vetted, then pinned while TLS still verifies hostname.
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .resolve_to_addrs(&host, &addresses)
                .build()
                .map_err(|_| denied())?;
            let method = match request.method {
                HttpMethod::GET => reqwest::Method::GET,
                HttpMethod::POST => reqwest::Method::POST,
                HttpMethod::PUT => reqwest::Method::PUT,
                HttpMethod::PATCH => reqwest::Method::PATCH,
                HttpMethod::DELETE => reqwest::Method::DELETE,
            };
            let mut outgoing = client
                .request(method, url)
                .header("User-Agent", format!("CodeMux-Addon/1 {plugin}"));
            for (key, value) in &request.headers {
                outgoing = outgoing.header(key, value)
            }
            if let Some(secret) = credential {
                outgoing = outgoing.bearer_auth(secret)
            }
            self.charge(request.body.as_ref().map_or(0, String::len))
                .await?;
            if let Some(body) = request.body {
                outgoing = outgoing.body(body)
            }
            let mut response = outgoing.send().await.map_err(|_| denied())?;
            if response.status().is_redirection() {
                return Err(denied());
            }
            // Compression is not requested. Refuse unsolicited encoding rather than
            // return compressed bytes or expose an unbounded decompression path.
            if response
                .headers()
                .get("content-encoding")
                .is_some_and(|v| v != "identity")
            {
                return Err(denied());
            }
            if response.content_length().is_some_and(|n| n > 512 * 1024) {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "HTTP response exceeds 512 KiB",
                ));
            }
            let status = response.status().as_u16();
            let mut headers = BTreeMap::new();
            for name in [
                "content-type",
                "etag",
                "retry-after",
                "x-ratelimit-limit",
                "x-ratelimit-remaining",
                "x-ratelimit-reset",
            ] {
                if let Some(value) = response
                    .headers()
                    .get(name)
                    .and_then(|h| h.to_str().ok())
                    .filter(|s| s.len() <= 4096)
                {
                    headers.insert(name.into(), value.into());
                }
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| denied())? {
                self.charge(chunk.len()).await?;
                if body.len() + chunk.len() > 512 * 1024 {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceLimit,
                        "HTTP response exceeds 512 KiB",
                    ));
                }
                body.extend_from_slice(&chunk)
            }
            let body = String::from_utf8(body)
                .map_err(|_| ProtocolError::invalid("HTTP body is not UTF-8"))?;
            Ok(Response {
                status,
                headers,
                body,
            })
        };
        tokio::select! {_ = cancel.cancelled()=>Err(ProtocolError::new(ErrorCode::PluginStopped,"Plugin request was cancelled")),result=tokio::time::timeout(Duration::from_secs(30),work)=>result.map_err(|_|ProtocolError::new(ErrorCode::Timeout,"HTTP request timed out"))?}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "Explicit network smoke: no credentials or workspace data; requires public HTTPS access"]
    async fn native_approved_public_https_request_uses_pinned_dns_and_verified_tls() {
        let grant = HttpGrant {
            origin: "https://api.github.com".into(),
            methods: vec![HttpMethod::GET],
            credential: None,
        };
        let response = Http::default()
            .fetch(
                "test.native-http",
                Request {
                    context: "fixture".into(),
                    origin: grant.origin.clone(),
                    path: "/zen".into(),
                    method: HttpMethod::GET,
                    headers: BTreeMap::new(),
                    body: None,
                },
                &grant,
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert!(!response.body.is_empty());
        assert!(response.body.len() < 512 * 1024);
        assert!(!response.headers.contains_key("set-cookie"));
    }
    #[test]
    fn ssrf_ranges_and_mapped_addresses_are_denied() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.64.0.1",
            "198.18.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fe80::1",
            "fc00::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}")
        }
        assert!(public_address("8.8.8.8".parse().unwrap()));
        assert!(public_address("2606:4700:4700::1111".parse().unwrap()));
    }
    #[test]
    fn headers_authority_and_methods_cannot_escape_the_grant() {
        let grant = HttpGrant {
            origin: "https://api.github.com".into(),
            methods: vec![HttpMethod::GET],
            credential: None,
        };
        let mut request = Request {
            context: "handle".into(),
            origin: grant.origin.clone(),
            path: "/repos/example/test/issues".into(),
            method: HttpMethod::GET,
            headers: BTreeMap::new(),
            body: None,
        };
        assert!(validate(&request, &grant).is_ok());
        request.path = "//localhost/secret".into();
        assert!(validate(&request, &grant).is_err());
        request.path = "/issues".into();
        request
            .headers
            .insert("Authorization".into(), "secret".into());
        assert!(validate(&request, &grant).is_err());
        request.headers.clear();
        request.method = HttpMethod::POST;
        assert!(validate(&request, &grant).is_err());
    }
}
