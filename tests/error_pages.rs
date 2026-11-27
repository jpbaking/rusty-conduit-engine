//! Proxy-generated errors are HTML pages rendered from the configured `ErrorPageTemplate`
//! on the forward, `CONNECT`, and decrypted MITM paths, while error responses from an
//! upstream origin server pass through with their status, headers, and body unchanged.

use std::sync::Arc;
use std::time::Duration;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{RootCertStore, ServerConfig};
use time::OffsetDateTime;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_util::sync::CancellationToken;

use rusty_conduit_engine::http::mitm_tunnel::client_config_with_roots;
use rusty_conduit_engine::http::ErrorPageTemplate;
use rusty_conduit_engine::logging::{AccessLogConfig, AccessLogger, LogFormat, LogLevel, Logger};
use rusty_conduit_engine::mitm::ca::CaMaterial;
use rusty_conduit_engine::mitm::signer::LeafSigner;
use rusty_conduit_engine::mitm::{crypto_provider, MitmOptions, MitmState};
use rusty_conduit_engine::routing::{RoutingConfig, RoutingInput, RoutingState};
use rusty_conduit_engine::server::{EngineHandlers, ListenPlan, Server};
use rusty_conduit_engine::upstream::DialerOptions;

const ROUTING: &str = r#"
default = "direct"

[[rules]]
name = "blocked-host"
host = "blocked.example"
target = "block"

[[rules]]
name = "blocked-path"
keyword = "blockme"
target = "block"
"#;

const TEMPLATE: &str = "<p>{{status}}|{{reason}}|{{message}}|{{target}}</p>";

/// Raw responses served by the origins: `/404` and `/500` are upstream errors.
const ORIGIN_404: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\
    X-Origin: yes\r\nContent-Length: 16\r\nConnection: close\r\n\r\norigin says 404!";
const ORIGIN_500: &[u8] = b"HTTP/1.1 500 Internal Server Error\r\n\
    Content-Type: application/json\r\nContent-Length: 17\r\nConnection: close\r\n\r\n\
    {\"error\":\"boom\"}\n";

fn quiet() -> Logger {
    Logger::with_writer(LogLevel::Quiet, Box::new(std::io::sink()))
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn parse(raw: &[u8]) -> Self {
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or_else(|| panic!("no header end in {:?}", String::from_utf8_lossy(raw)));
        let head = std::str::from_utf8(&raw[..split]).unwrap();
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap().split(' ').nth(1).unwrap();
        let headers = lines
            .map(|l| {
                let (k, v) = l.split_once(':').unwrap();
                (k.trim().to_ascii_lowercase(), v.trim().to_string())
            })
            .collect();
        Self {
            status: status.parse().unwrap(),
            headers,
            body: raw[split + 4..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }

    fn assert_page(&self, status: u16, expected_body: &str) {
        assert_eq!(self.status, status, "{}", self.text());
        assert_eq!(
            self.header("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(self.header("cache-control"), Some("no-store"));
        assert_eq!(self.text(), expected_body);
    }

    /// The origin's response, byte for byte apart from the stripped `Connection` header.
    fn assert_origin(&self, raw: &[u8]) {
        let origin = Self::parse(raw);
        assert_eq!(self.status, origin.status);
        assert_eq!(self.body, origin.body);
        let kept: Vec<_> = origin
            .headers
            .iter()
            .filter(|(k, _)| k != "connection")
            .cloned()
            .collect();
        let mut got: Vec<_> = self
            .headers
            .iter()
            .filter(|(k, _)| k != "date")
            .cloned()
            .collect();
        got.retain(|(k, _)| k != "connection");
        assert_eq!(got, kept);
        assert_eq!(self.header("cache-control"), None);
    }
}

async fn read_request<S: AsyncRead + Unpin>(s: &mut S) -> String {
    let mut buf = Vec::new();
    while !buf.ends_with(b"\r\n\r\n") {
        match s.read_u8().await {
            Ok(b) => buf.push(b),
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

async fn answer<S: AsyncRead + AsyncWrite + Unpin>(mut s: S) {
    let req = read_request(&mut s).await;
    let raw = if req.starts_with("GET /500 ") {
        ORIGIN_500
    } else {
        ORIGIN_404
    };
    let _ = s.write_all(raw).await;
    let _ = s.shutdown().await;
}

async fn http_origin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            tokio::spawn(answer(s));
        }
    });
    port
}

/// A port with nothing listening on it.
async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

async fn start_proxy(handlers: EngineHandlers) -> (std::net::SocketAddr, CancellationToken) {
    let server = Server::bind(
        ListenPlan::Separate {
            http: Some("127.0.0.1:0".parse().unwrap()),
            socks: None,
        },
        Arc::new(handlers),
        quiet(),
    )
    .await
    .unwrap();
    let addr = server.local_http_addr().unwrap();
    let token = server.shutdown_token();
    tokio::spawn(server.run());
    (addr, token)
}

async fn handlers(page: Option<ErrorPageTemplate>) -> EngineHandlers {
    let input = RoutingInput {
        routing: toml::from_str::<RoutingConfig>(ROUTING).unwrap(),
        ..RoutingInput::default()
    };
    input.validate().unwrap();
    let state = RoutingState::from_input(&input, DialerOptions::default())
        .unwrap()
        .with_connect_timeout(Duration::from_secs(3));
    let access = AccessLogger::start(
        AccessLogConfig::new(LogLevel::Quiet, LogFormat::Text, None, None),
        quiet(),
    )
    .await
    .unwrap();
    let handlers = EngineHandlers::from_resolver(state.shared(), access, quiet());
    match page {
        Some(p) => handlers.with_error_page(p),
        None => handlers,
    }
}

/// Read one response (headers plus a `Content-Length` body): the proxy may keep the client
/// connection open after it.
async fn read_response<S: AsyncRead + Unpin>(s: &mut S) -> Response {
    let read = async {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = Response::parse(&raw[..end + 4]);
                let len: usize = head
                    .header("content-length")
                    .map_or(0, |v| v.parse().unwrap());
                if raw.len() >= end + 4 + len {
                    return raw;
                }
            }
            match s.read(&mut chunk).await {
                Ok(0) | Err(_) => return raw,
                Ok(n) => raw.extend_from_slice(&chunk[..n]),
            }
        }
    };
    let raw = tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .expect("response timed out");
    Response::parse(&raw)
}

async fn send(proxy: std::net::SocketAddr, request: &str) -> Response {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    read_response(&mut s).await
}

fn get(url: &str, host: &str) -> String {
    format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn forward_proxy_errors_use_the_template_and_origin_errors_pass_through() {
    let origin = http_origin().await;
    let closed = closed_port().await;
    let (proxy, token) =
        start_proxy(handlers(Some(ErrorPageTemplate::from_template(TEMPLATE))).await).await;

    let host = format!("127.0.0.1:{origin}");
    send(proxy, &get(&format!("http://{host}/404"), &host))
        .await
        .assert_origin(ORIGIN_404);
    send(proxy, &get(&format!("http://{host}/500"), &host))
        .await
        .assert_origin(ORIGIN_500);

    send(proxy, &get("http://blocked.example/x", "blocked.example"))
        .await
        .assert_page(
            403,
            "<p>403|Forbidden|upstream connection failed|blocked.example</p>",
        );
    let dead = format!("127.0.0.1:{closed}");
    send(proxy, &get(&format!("http://{dead}/"), &dead))
        .await
        .assert_page(
            502,
            &format!("<p>502|Bad Gateway|upstream connection failed|{dead}</p>"),
        );
    // Credentials in the request URI never reach the page.
    send(
        proxy,
        &get("https://user:pw@secure.example/", "secure.example"),
    )
    .await
    .assert_page(
        400,
        "<p>400|Bad Request|only http:// URIs can be forwarded (use CONNECT for https)|\
             secure.example</p>",
    );
    token.cancel();
}

#[tokio::test]
async fn connect_errors_use_the_builtin_page_by_default() {
    let (proxy, token) = start_proxy(handlers(None).await).await;

    let resp = send(
        proxy,
        "CONNECT blocked.example:443 HTTP/1.1\r\nHost: blocked.example:443\r\n\r\n",
    )
    .await;
    assert_eq!(resp.status, 403);
    assert_eq!(
        resp.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(resp.header("cache-control"), Some("no-store"));
    let body = resp.text();
    assert!(body.starts_with("<!DOCTYPE html>"), "{body}");
    assert!(body.contains("<h1>403 Forbidden</h1>"));
    assert!(body.contains("<p>upstream connection failed</p>"));
    assert!(body.contains("<small>blocked.example:443</small>"));
    token.cancel();
}

#[tokio::test]
async fn connect_errors_use_the_configured_template() {
    let closed = closed_port().await;
    let (proxy, token) =
        start_proxy(handlers(Some(ErrorPageTemplate::from_template(TEMPLATE))).await).await;
    let dead = format!("127.0.0.1:{closed}");
    send(
        proxy,
        &format!("CONNECT {dead} HTTP/1.1\r\nHost: {dead}\r\n\r\n"),
    )
    .await
    .assert_page(
        502,
        &format!("<p>502|Bad Gateway|upstream connection failed|{dead}</p>"),
    );
    token.cancel();
}

// --- MITM -------------------------------------------------------------------------------

fn test_ca(cn: &str) -> (String, String, Vec<u8>) {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, cn);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let now = OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(365);
    let cert = params.self_signed(&key).unwrap();
    (cert.pem(), key.serialize_pem(), cert.der().to_vec())
}

#[derive(Debug)]
struct FixedCert(Arc<CertifiedKey>);

impl ResolvesServerCert for FixedCert {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// HTTPS/1.1 origin for `localhost` serving [`ORIGIN_404`] / [`ORIGIN_500`].
async fn https_origin() -> (u16, CertificateDer<'static>) {
    let (cert_pem, key_pem, ca_der) = test_ca("Error Page Origin CA");
    let ca = CaMaterial::from_pem_bytes(cert_pem.as_bytes(), key_pem.as_bytes(), None).unwrap();
    let leaf = LeafSigner::new(&ca, Duration::from_secs(3600), crypto_provider())
        .unwrap()
        .sign_at("localhost", OffsetDateTime::now_utc())
        .unwrap();
    let mut cfg = ServerConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedCert(leaf.certified)));
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(cfg));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    answer(tls).await;
                }
            });
        }
    });
    (port, CertificateDer::from(ca_der))
}

/// One decrypted `GET path` through a fresh MITM `CONNECT`.
async fn mitm_get(
    proxy: std::net::SocketAddr,
    authority: &str,
    mitm_ca: &[u8],
    path: &str,
) -> Response {
    let mut tcp = TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert!(read_request(&mut tcp).await.contains(" 200"));
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(mitm_ca.to_vec())).unwrap();
    let mut cfg = client_config_with_roots(crypto_provider(), roots).unwrap();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let mut tls = TlsConnector::from(Arc::new(cfg))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    tls.write_all(get(path, authority).as_bytes())
        .await
        .unwrap();
    read_response(&mut tls).await
}

async fn mitm_proxy(
    engine_page: Option<ErrorPageTemplate>,
    interceptor_page: Option<ErrorPageTemplate>,
) -> (u16, std::net::SocketAddr, CancellationToken, Vec<u8>) {
    let (origin_port, origin_ca) = https_origin().await;
    let (mitm_cert, mitm_key, mitm_ca_der) = test_ca("Error Page MITM CA");
    let mut origin_roots = RootCertStore::empty();
    origin_roots.add(origin_ca).unwrap();
    let upstream = client_config_with_roots(crypto_provider(), origin_roots).unwrap();
    let mitm = MitmState::from_pem_bytes_with_upstream(
        mitm_cert.as_bytes(),
        mitm_key.as_bytes(),
        &MitmOptions::default(),
        crypto_provider(),
        upstream,
    )
    .unwrap();
    let interceptor = match interceptor_page {
        Some(p) => mitm.interceptor.with_error_page(p),
        None => mitm.interceptor,
    };
    let handlers = handlers(engine_page)
        .await
        .with_mitm(Arc::new(interceptor), false);
    let (proxy, token) = start_proxy(handlers).await;
    (origin_port, proxy, token, mitm_ca_der)
}

#[tokio::test]
async fn mitm_errors_use_the_engine_template_and_origin_errors_pass_through() {
    let (port, proxy, token, ca) =
        mitm_proxy(Some(ErrorPageTemplate::from_template(TEMPLATE)), None).await;
    let authority = format!("localhost:{port}");

    mitm_get(proxy, &authority, &ca, "/404")
        .await
        .assert_origin(ORIGIN_404);
    mitm_get(proxy, &authority, &ca, "/500")
        .await
        .assert_origin(ORIGIN_500);
    mitm_get(proxy, &authority, &ca, "/blockme")
        .await
        .assert_page(
            403,
            &format!("<p>403|Forbidden|destination blocked|{authority}</p>"),
        );
    token.cancel();
}

#[tokio::test]
async fn mitm_interceptor_template_overrides_the_engine_template() {
    let (port, proxy, token, ca) = mitm_proxy(
        Some(ErrorPageTemplate::from_template("engine")),
        Some(ErrorPageTemplate::from_template(
            "mitm {{status}} {{target}}",
        )),
    )
    .await;
    let authority = format!("localhost:{port}");
    mitm_get(proxy, &authority, &ca, "/blockme")
        .await
        .assert_page(403, &format!("mitm 403 {authority}"));
    token.cancel();
}
