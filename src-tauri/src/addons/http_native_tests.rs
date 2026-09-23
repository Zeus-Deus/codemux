use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{rustls, TlsAcceptor};

fn grant() -> HttpGrant {
    HttpGrant {
        origin: "https://addon-http.invalid".into(),
        methods: vec![HttpMethod::GET],
        credential: None,
    }
}
fn request() -> Request {
    Request {
        context: "fixture".into(),
        origin: grant().origin,
        path: "/fixture".into(),
        method: HttpMethod::GET,
        headers: BTreeMap::new(),
        body: None,
    }
}
async fn server(
    response: Vec<u8>,
    delay: Duration,
) -> (
    Http,
    tokio::task::JoinHandle<String>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                include_bytes!("test-fixtures/http/server.der").to_vec(),
            )],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                include_bytes!("test-fixtures/http/server-key.der").to_vec(),
            )),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let connect = listener.local_addr().unwrap();
    let (received, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let Ok(mut stream) = TlsAcceptor::from(Arc::new(config)).accept(tcp).await else {
            return String::new();
        };
        let mut headers = Vec::new();
        loop {
            let Ok(byte) = stream.read_u8().await else {
                return String::new();
            };
            headers.push(byte);
            assert!(headers.len() <= 16 * 1024);
            if headers.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let _ = received.send(());
        tokio::time::sleep(delay).await;
        let _ = stream.write_all(&response).await;
        let _ = stream.shutdown().await;
        String::from_utf8(headers).unwrap()
    });
    let http = Http {
        native_fixture: Some(NativeFixture {
            answers: vec!["8.8.8.8:443".parse().unwrap()],
            connect,
            trust_certificate: true,
            timeout: Duration::from_secs(2),
        }),
        ..Http::default()
    };
    (http, task, ready)
}
async fn finish(task: tokio::task::JoinHandle<String>) -> String {
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn pinned_native_tls_preserves_hostname_credentials_and_safe_response_headers() {
    let (http, task, _) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nX-RateLimit-Remaining: 59\r\nETag: fixture\r\nSet-Cookie: hidden=value\r\nConnection: close\r\n\r\nok".to_vec(), Duration::ZERO).await;
    let response = http
        .fetch(
            "test.http",
            request(),
            &grant(),
            Some("synthetic-test-token".into()),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(response.body, "ok");
    // Only content-type and rate-limit headers are returned (chapter 6).
    assert_eq!(
        response.headers,
        BTreeMap::from([
            ("content-type".to_string(), "text/plain".to_string()),
            ("x-ratelimit-remaining".into(), "59".into()),
        ])
    );
    let headers = finish(task).await.to_ascii_lowercase();
    assert!(headers.contains("host: addon-http.invalid"));
    assert!(headers.contains("authorization: bearer synthetic-test-token"));
    assert!(headers.contains("user-agent: codemux-addon/1 test.http\r\n"));
    assert!(!headers.contains("cookie"));
    // .invalid has no public DNS: a successful real TLS connection proves the
    // client uses the pinned endpoint, not a second lookup of this hostname.
}
#[tokio::test]
async fn untrusted_certificate_and_rebound_private_dns_are_rejected() {
    let (mut http, task, _) = server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
        Duration::ZERO,
    )
    .await;
    http.native_fixture.as_mut().unwrap().trust_certificate = false;
    assert!(http
        .fetch(
            "test.http",
            request(),
            &grant(),
            None,
            &CancellationToken::new()
        )
        .await
        .is_err());
    assert!(finish(task).await.is_empty());
    let (other_http, other_task, _) = server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
        Duration::ZERO,
    )
    .await;
    let mut other_grant = grant();
    other_grant.origin = "https://wrong-host.invalid".into();
    let mut other_request = request();
    other_request.origin = other_grant.origin.clone();
    assert!(other_http
        .fetch(
            "test.http",
            other_request,
            &other_grant,
            None,
            &CancellationToken::new()
        )
        .await
        .is_err());
    assert!(finish(other_task).await.is_empty());
    for addresses in [
        vec![],
        vec!["127.0.0.1:443"],
        vec!["8.8.8.8:443", "[::ffff:127.0.0.1]:443"],
    ] {
        http.native_fixture.as_mut().unwrap().answers =
            addresses.iter().map(|s| s.parse().unwrap()).collect();
        let result = http
            .fetch(
                "test.http",
                request(),
                &grant(),
                None,
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(
            result,
            Err(error) if error.data.code == ErrorCode::NetworkDenied
        ));
    }
}
#[tokio::test]
async fn native_redirect_encoding_and_streamed_body_limits_are_enforced() {
    let cases = [
        b"HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbomb".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 524289\r\nConnection: close\r\n\r\n".to_vec(),
        {
            let mut response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n80001\r\n".to_vec();
            response.extend(vec![b'x'; 512 * 1024 + 1]);
            response.extend_from_slice(b"\r\n0\r\n\r\n");
            response
        },
    ];
    for response in cases {
        let (http, task, _) = server(response, Duration::ZERO).await;
        assert!(http
            .fetch(
                "test.http",
                request(),
                &grant(),
                None,
                &CancellationToken::new()
            )
            .await
            .is_err());
        finish(task).await;
    }
}
#[tokio::test]
async fn native_timeout_cancellation_and_concurrency_release_the_request_slot() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let (mut http, task, ready) = server(response.clone(), Duration::from_millis(1000)).await;
    http.native_fixture.as_mut().unwrap().timeout = Duration::from_millis(500);
    let began = Instant::now();
    let request_future = http
        .fetch(
            "test.http",
            request(),
            &grant(),
            None,
            &CancellationToken::new(),
        )
        .await;
    assert!(request_future.is_err());
    assert!(began.elapsed() < Duration::from_secs(2));
    assert_eq!(http.slots.available_permits(), 4);
    drop(ready);
    finish(task).await;
    let (http, task, ready) = server(response, Duration::from_millis(100)).await;
    let cancel = CancellationToken::new();
    let cloned = http.clone();
    let child_cancel = cancel.clone();
    let fetch = tokio::spawn(async move {
        cloned
            .fetch("test.http", request(), &grant(), None, &child_cancel)
            .await
    });
    ready.await.unwrap();
    cancel.cancel();
    assert!(matches!(
        fetch.await.unwrap(),
        Err(error) if error.data.code == ErrorCode::PluginStopped
    ));
    assert_eq!(http.slots.available_permits(), 4);
    finish(task).await;
    let permits = http.slots.acquire_many(4).await.unwrap();
    assert!(matches!(
        http.fetch(
            "test.http",
            request(),
            &grant(),
            None,
            &CancellationToken::new()
        )
        .await,
        Err(error) if error.data.code == ErrorCode::ResourceLimit
    ));
    drop(permits);
}

#[tokio::test]
async fn rolling_network_quota_is_private_and_recovers_after_its_window() {
    let http = Http::default();
    http.charge(10 * 1024 * 1024).await.unwrap();
    assert!(http.charge(1).await.is_err());
    Http::default().charge(1).await.unwrap();
    {
        let mut traffic = http.traffic.lock().await;
        traffic.entries.front_mut().unwrap().0 = Instant::now() - Duration::from_secs(62);
    }
    http.charge(1).await.unwrap();
    assert_eq!(http.traffic.lock().await.total, 1);
}
