//! Bounded native GitHub API transport. Credentials are supplied by its owner.
use reqwest::header::HeaderMap;
use reqwest::{Method, Url};
use std::time::Duration;

pub(super) struct Request {
    url: Url,
    method: Method,
    headers: HeaderMap,
    body: Option<String>,
}

pub(super) struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: String,
}

pub(super) fn send(
    client: &reqwest::blocking::Client,
    request: Request,
    token: &str,
    timeout: Duration,
    validator: Option<&str>,
) -> Result<Response, String> {
    use std::io::Read;
    const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
    let has_accept = request.headers.contains_key(reqwest::header::ACCEPT);
    let is_get = request.method == Method::GET;
    let mut builder = client
        .request(request.method, request.url)
        .headers(request.headers)
        .header(reqwest::header::USER_AGENT, "codemux")
        .header("x-github-api-version", "2022-11-28")
        .bearer_auth(token)
        .timeout(timeout);
    if !has_accept {
        builder = builder.header(reqwest::header::ACCEPT, "application/vnd.github+json");
    }
    if let Some(body) = request.body {
        builder = builder
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    if let Some(validator) = validator.filter(|_| is_get) {
        builder = builder.header(reqwest::header::IF_NONE_MATCH, validator);
    }
    let response = builder.send().map_err(|error| {
        if error.is_timeout() {
            "GitHub API request timed out"
        } else {
            "GitHub API request failed"
        }
    })?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err("GitHub API response exceeded the size limit".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut {
                "GitHub API response timed out"
            } else {
                "Could not read GitHub API response"
            }
        })?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("GitHub API response exceeded the size limit".into());
    }
    let body = String::from_utf8(bytes).map_err(|_| "GitHub API response was not valid UTF-8")?;
    Ok(Response {
        status,
        headers,
        body,
    })
}

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn valid_host(host: &str) -> bool {
    if host.is_empty()
        || host.len() > 259
        || host
            .bytes()
            .any(|c| !c.is_ascii_alphanumeric() && !b".-:".contains(&c))
    {
        return false;
    }
    let (name, port) = host
        .split_once(':')
        .map_or((host, None), |(name, port)| (name, Some(port)));
    if port.is_some_and(|p| p.is_empty() || p.parse::<u16>().ok().is_none_or(|n| n == 0)) {
        return false;
    }
    name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

fn endpoint_url(host: &str, endpoint: &str) -> Result<Url, String> {
    if !valid_host(host) {
        return Err("Invalid GitHub API hostname".into());
    }
    // Validate the raw input BEFORE URL parsing can normalize traversal,
    // controls, backslashes or an alternate authority. Only local API shapes
    // are supported; an absolute URL never inherits our credential.
    if endpoint.is_empty()
        || endpoint.starts_with('/')
        || endpoint.contains("://")
        || endpoint.contains(['\\', '#'])
        || endpoint.bytes().any(|c| c <= 0x20 || c == 0x7f)
    {
        return Err("Invalid GitHub API endpoint".into());
    }
    let path = endpoint.split('?').next().unwrap();
    let lower = path.to_ascii_lowercase();
    if path.split('/').any(|s| matches!(s, "." | ".." | ""))
        || ["%2e", "%2f", "%5c", "%25", "%00"]
            .iter()
            .any(|s| lower.contains(s))
    {
        return Err("Invalid GitHub API endpoint".into());
    }
    let cloud = host.eq_ignore_ascii_case("github.com") || host.ends_with(".ghe.com");
    let base = if host.eq_ignore_ascii_case("github.com") {
        "https://api.github.com/".to_string()
    } else if cloud {
        format!("https://api.{host}/")
    } else {
        format!("https://{host}/api/v3/")
    };
    Url::parse(&if endpoint == "graphql" && !cloud {
        format!("https://{host}/api/graphql")
    } else {
        format!("{base}{endpoint}")
    })
    .map_err(|_| "Invalid GitHub API URL".into())
}

pub(super) fn parse_request(
    host: &str,
    args: &[&str],
    stdin: Option<&str>,
) -> Result<Request, String> {
    if args.first() != Some(&"api") {
        return Err("Expected GitHub API request".into());
    }
    if args.len() > 128
        || args.iter().map(|a| a.len()).sum::<usize>() > MAX_BODY_BYTES
        || stdin.is_some_and(|s| s.len() > MAX_BODY_BYTES)
    {
        return Err("GitHub API request exceeded the size limit".into());
    }
    let mut endpoint = None;
    let mut method = None;
    let mut headers = HeaderMap::new();
    let mut fields = serde_json::Map::new();
    let mut input = false;
    let mut i = 1;
    while i < args.len() {
        let arg = args[i];
        let (name, value) = if matches!(
            arg,
            "-X" | "--method"
                | "-H"
                | "--header"
                | "-f"
                | "--raw-field"
                | "-F"
                | "--field"
                | "--hostname"
                | "--input"
        ) {
            i += 1;
            (arg, *args.get(i).ok_or("Missing GitHub API option value")?)
        } else if let Some((name, value)) = arg
            .split_once('=')
            .filter(|(name, _)| name.starts_with("--"))
        {
            (name, value)
        } else if let Some(name) = ["-X", "-H", "-f", "-F"]
            .iter()
            .find(|name| arg.starts_with(**name))
        {
            (*name, &arg[name.len()..])
        } else if !arg.starts_with('-') && endpoint.is_none() {
            endpoint = Some(arg);
            i += 1;
            continue;
        } else {
            return Err("Unsupported GitHub API option".into());
        };
        match name {
            "-X" | "--method" => {
                method = Some(
                    Method::from_bytes(value.as_bytes())
                        .map_err(|_| "Invalid GitHub API method")?,
                )
            }
            "-H" | "--header" => {
                let (name, value) = value.split_once(':').ok_or("Invalid GitHub API header")?;
                if !matches!(
                    name.trim().to_ascii_lowercase().as_str(),
                    "accept" | "x-github-api-version" | "if-none-match" | "if-modified-since"
                ) {
                    return Err("Unsupported GitHub API header".into());
                }
                headers.insert(
                    reqwest::header::HeaderName::from_bytes(name.trim().as_bytes())
                        .map_err(|_| "Invalid GitHub API header")?,
                    reqwest::header::HeaderValue::from_str(value.trim())
                        .map_err(|_| "Invalid GitHub API header")?,
                );
            }
            "-f" | "--raw-field" | "-F" | "--field" => {
                let typed = matches!(name, "-F" | "--field");
                let (name, value) = value.split_once('=').ok_or("Invalid GitHub API field")?;
                if typed && value.starts_with('@') {
                    return Err("GitHub API file fields are not supported".into());
                }
                fields.insert(
                    name.to_string(),
                    if typed {
                        serde_json::from_str(value)
                            .unwrap_or_else(|_| serde_json::Value::String(value.to_string()))
                    } else {
                        serde_json::Value::String(value.to_string())
                    },
                );
            }
            "--hostname" => {
                if !value.eq_ignore_ascii_case(host) {
                    return Err("GitHub API host mismatch".into());
                }
            }
            "--input" => {
                if value != "-" || stdin.is_none() {
                    return Err("Only supplied GitHub JSON input is supported".into());
                }
                input = true;
            }
            _ => return Err("Unsupported GitHub API option".into()),
        }
        i += 1;
    }
    let endpoint = endpoint.ok_or("Missing GitHub API endpoint")?;
    let graphql = endpoint == "graphql";
    let mut url = endpoint_url(host, endpoint)?;
    let method = method.unwrap_or(if input || !fields.is_empty() {
        Method::POST
    } else {
        Method::GET
    });
    let body = if input {
        let body = stdin.ok_or("Missing GitHub JSON input")?;
        serde_json::from_str::<serde_json::Value>(body).map_err(|_| "Invalid GitHub JSON input")?;
        if !fields.is_empty() {
            return Err("Cannot combine GitHub JSON input and fields".into());
        }
        Some(body.to_string())
    } else if method == Method::GET {
        if !fields.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in fields {
                let value = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                pairs.append_pair(&name, &value);
            }
        }
        None
    } else if fields.is_empty() {
        None
    } else {
        let value = if graphql {
            let query = fields
                .remove("query")
                .ok_or("Missing GitHub GraphQL query")?;
            serde_json::json!({"query": query, "variables": fields})
        } else {
            serde_json::Value::Object(fields)
        };
        Some(value.to_string())
    };
    Ok(Request {
        url,
        method,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn fixture(response: String) -> (Url, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = Url::parse(&format!("http://{}/api", listener.local_addr().unwrap())).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut raw = Vec::new();
                        loop {
                            let mut buffer = [0; 4096];
                            let n = stream.read(&mut buffer).unwrap();
                            if n == 0 {
                                break;
                            }
                            raw.extend_from_slice(&buffer[..n]);
                            let text = String::from_utf8_lossy(&raw);
                            if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                                let length = headers
                                    .lines()
                                    .find_map(|line| {
                                        let (name, value) = line.split_once(':')?;
                                        name.eq_ignore_ascii_case("content-length")
                                            .then(|| value.trim().parse::<usize>().unwrap())
                                    })
                                    .unwrap_or(0);
                                if body.len() >= length {
                                    break;
                                }
                            }
                        }
                        let _ = tx.send(String::from_utf8(raw).unwrap());
                        let _ = stream.write_all(response.as_bytes());
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(_) => return,
                }
            }
        });
        (url, rx)
    }

    #[test]
    fn native_transport_sends_typed_graphql_and_returns_quota_headers() {
        let body = r#"{"data":{"viewer":{"login":"fixture"}}}"#;
        let (url, captured) = fixture(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-RateLimit-Resource: graphql\r\nX-RateLimit-Remaining: 4321\r\nConnection: close\r\n\r\n{body}", body.len()));
        let mut request = parse_request(
            "github.com",
            &[
                "api",
                "graphql",
                "-f",
                "query=query($number:Int!){viewer{login}}",
                "-F",
                "number=42",
            ],
            None,
        )
        .unwrap();
        request.url = url;
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = send(
            &client,
            request,
            "synthetic-test-token",
            Duration::from_secs(1),
            None,
        )
        .unwrap();
        assert_eq!(response.body, body);
        assert_eq!(response.status, 200);
        assert_eq!(response.headers["x-ratelimit-remaining"], "4321");
        let raw = captured.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(raw
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-test-token"));
        let sent: serde_json::Value =
            serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(sent["variables"]["number"], 42);
    }

    #[test]
    fn native_transport_preserves_conditional_responses_without_following_redirects() {
        for status in ["304 Not Modified", "302 Found"] {
            let (url, captured) = fixture(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nLocation: https://untrusted.invalid/token\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n"));
            let mut request =
                parse_request("github.com", &["api", "repos/fixture/repo"], None).unwrap();
            request.url = url;
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let response = send(
                &client,
                request,
                "synthetic-test-token",
                Duration::from_secs(1),
                Some("\"fixture\""),
            )
            .unwrap();
            assert_eq!(response.status.to_string(), &status[..3]);
            assert!(response.body.is_empty());
            let raw = captured.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(raw
                .to_ascii_lowercase()
                .contains("if-none-match: \"fixture\""));
        }
    }

    #[test]
    fn rejects_unsafe_authorities_endpoints_headers_and_unbounded_payloads() {
        for host in [
            "user:secret@github.com",
            "github.com/path",
            "github.com?x=y",
            "github.com#x",
            "github.com:wrong",
            "github.com\n",
            "github.com:",
            "github..com",
        ] {
            assert!(
                parse_request(host, &["api", "rate_limit"], None).is_err(),
                "host admitted: {host:?}"
            );
        }
        for endpoint in [
            "//evil.example/a",
            "https://evil.example/a",
            "../a",
            "repos/%2e%2e/a",
            "repos/a/%2Fescape",
            "repos/a\\b",
            "repos/a#fragment",
            "repos/a\n",
        ] {
            assert!(
                parse_request("github.com", &["api", endpoint], None).is_err(),
                "endpoint admitted: {endpoint:?}"
            );
        }
        for header in [
            "Authorization: secret",
            "Host: evil.example",
            "Cookie: private",
            "Accept: okay\r\nX-Secret: bad",
        ] {
            assert!(
                parse_request("github.com", &["api", "rate_limit", "-H", header], None).is_err()
            );
        }
        let large = "x".repeat(2 * 1024 * 1024 + 1);
        assert!(parse_request(
            "github.com",
            &["api", "repos/a/b", "--input", "-"],
            Some(&large)
        )
        .is_err());
        assert!(parse_request(
            "github.com",
            &["api", "repos/a/b", "--input", "-"],
            Some("not json")
        )
        .is_err());
        assert!(parse_request(
            "github.com",
            &["api", "repos/a/b", "-F", "body=@/private/token"],
            None
        )
        .is_err());
        let post = parse_request(
            "github.com",
            &[
                "api",
                "--method",
                "POST",
                "repos/a/b/comments",
                "--input",
                "-",
            ],
            Some(r#"{"body":"note"}"#),
        )
        .unwrap();
        assert_eq!(post.method, Method::POST);
        assert_eq!(post.body.as_deref(), Some(r#"{"body":"note"}"#));
        let get = parse_request(
            "github.com",
            &["api", "repos/a/b", "-XGET", "-F", "page=3"],
            None,
        )
        .unwrap();
        assert_eq!(get.url.query(), Some("page=3"));
        assert!(get.body.is_none());
    }
    #[test]
    fn parses_rest_get_and_enterprise_graphql_typed_variables() {
        let get = parse_request(
            "github.com",
            &[
                "api",
                "repos/a/b/comments?per_page=5",
                "-H",
                "Accept: application/vnd.github+json",
            ],
            None,
        )
        .unwrap();
        assert_eq!(get.method, Method::GET);
        assert_eq!(
            get.url.as_str(),
            "https://api.github.com/repos/a/b/comments?per_page=5"
        );
        assert!(get.body.is_none());
        let query = parse_request(
            "github.example:8443",
            &[
                "api",
                "graphql",
                "-f",
                "owner=a",
                "-F",
                "number=7",
                "-f",
                "query=query($number:Int!){viewer{login}}",
            ],
            None,
        )
        .unwrap();
        assert_eq!(query.method, Method::POST);
        assert_eq!(
            query.url.as_str(),
            "https://github.example:8443/api/graphql"
        );
        let body: serde_json::Value = serde_json::from_str(query.body.as_ref().unwrap()).unwrap();
        assert_eq!(body["variables"]["number"], 7);
        assert_eq!(body["variables"]["owner"], "a");
        assert_eq!(body["query"], "query($number:Int!){viewer{login}}");
    }
}
