//! Fixtures for consumers' tests, behind the opt-in `test-support` feature: test CA, echo
//! server, HTTP origin, TLS client, and logger capture. Not covered by semver stability.

use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1 as server_http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{RootCertStore, ServerConfig};
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::http::mitm_tunnel::client_config_with_roots;
use crate::logging::{AccessLogConfig, AccessLogger, LogFormat, LogLevel, Logger};
use crate::mitm::ca::CaMaterial;
use crate::mitm::crypto_provider;
use crate::mitm::signer::LeafSigner;

// ---------------------------------------------------------------------------
// Access-log capture
// ---------------------------------------------------------------------------

/// In-memory async writer capturing access-log output.
#[derive(Clone, Default)]
pub struct Buf(pub Arc<Mutex<Vec<u8>>>);

impl Buf {
    pub fn lines(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Wait (up to 5s) until at least `n` lines were logged.
    pub async fn wait_lines(&self, n: usize) -> Vec<String> {
        for _ in 0..500 {
            let lines = self.lines();
            if lines.len() >= n {
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("expected {n} access log lines, got {:?}", self.lines());
    }
}

impl AsyncWrite for Buf {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, b: &[u8]) -> Poll<io::Result<usize>> {
        self.0.lock().unwrap().extend_from_slice(b);
        Poll::Ready(Ok(b.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub fn quiet_logger() -> Logger {
    Logger::with_writer(LogLevel::Quiet, Box::new(io::sink()))
}

/// An access logger dumping text records into the returned buffer.
pub async fn capture_access() -> (AccessLogger, Buf) {
    let buf = Buf::default();
    let cfg = AccessLogConfig {
        format: LogFormat::Text,
        http_access_log: None,
        socks_log: None,
        dump_stdout: true,
        include_routing_fields: false,
    };
    let access = AccessLogger::start_with_stdout(cfg, Box::new(buf.clone()), quiet_logger())
        .await
        .unwrap();
    (access, buf)
}

// ---------------------------------------------------------------------------
// Echo upstream
// ---------------------------------------------------------------------------

/// Echo server on `bind` (e.g. `127.0.0.1:0`).
pub async fn spawn_echo(bind: &str) -> SocketAddr {
    let listener = TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = sock.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// Send `payload` through an established tunnel, half-close, and read everything back.
pub async fn echo_roundtrip(mut stream: TcpStream, payload: &[u8]) -> Vec<u8> {
    stream.write_all(payload).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    out
}

// ---------------------------------------------------------------------------
// Test CA and TLS origin
// ---------------------------------------------------------------------------

/// An ephemeral CA as PEM strings.
pub struct TestCa {
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_der: Vec<u8>,
}

/// A currently valid ephemeral ECDSA P-256 CA.
pub fn test_ca() -> TestCa {
    let now = OffsetDateTime::now_utc();
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "Rusty Conduit Test CA");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(365);
    let cert = params.self_signed(&key).unwrap();
    TestCa {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
        cert_der: cert.der().to_vec(),
    }
}

#[derive(Debug)]
struct FixedCert(Arc<CertifiedKey>);

impl ResolvesServerCert for FixedCert {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// A TLS origin on 127.0.0.1 (leaf for `localhost`, signed by its own CA).
pub struct Origin {
    pub addr: SocketAddr,
    pub ca_der: CertificateDer<'static>,
}

/// HTTP/1.1 TLS origin answering `hit <n> <method> <path> <body-len>`.
pub async fn spawn_origin() -> Origin {
    let ca = test_ca();
    let material =
        CaMaterial::from_pem(ca.cert_pem.as_bytes(), ca.key_pem.as_bytes(), None).unwrap();
    let signer = LeafSigner::new(&material, Duration::from_secs(3600), crypto_provider()).unwrap();
    let leaf = signer
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
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let hits = Arc::clone(&hits);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let service = service_fn(move |req: Request<Incoming>| {
                    let n = hits.fetch_add(1, Ordering::SeqCst) + 1;
                    async move {
                        let method = req.method().to_string();
                        let path = req.uri().path().to_string();
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(format!(
                            "hit {n} {method} {path} {}",
                            body.len()
                        )))))
                    }
                });
                let _ = server_http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service)
                    .await;
            });
        }
    });
    Origin {
        addr,
        ca_der: CertificateDer::from(ca.cert_der),
    }
}

/// `CONNECT target` through `proxy`; returns the raw stream after `200`.
pub async fn connect_through(proxy: SocketAddr, target: &str) -> TcpStream {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    stream
        .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).await.unwrap();
    assert_eq!(status, "HTTP/1.1 200 Connection Established\r\n");
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        if line == "\r\n" {
            break;
        }
    }
    assert!(reader.buffer().is_empty());
    reader.into_inner()
}

/// TLS client trusting only the MITM CA.
pub async fn tls_client(
    stream: TcpStream,
    mitm_ca: &TestCa,
    sni: &str,
    alpn: &[&[u8]],
) -> io::Result<ClientTlsStream<TcpStream>> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(mitm_ca.cert_der.clone()))
        .unwrap();
    let mut cfg = client_config_with_roots(crypto_provider(), roots).unwrap();
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    TlsConnector::from(Arc::new(cfg))
        .connect(ServerName::try_from(sni.to_string()).unwrap(), stream)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fixtures_work() {
        let _logger = quiet_logger();
        let (access, buf) = capture_access().await;
        drop(access);
        assert!(buf.lines().is_empty());

        let echo_addr = spawn_echo("127.0.0.1:0").await;
        let stream = TcpStream::connect(echo_addr).await.unwrap();
        let echoed = echo_roundtrip(stream, b"hello conduit").await;
        assert_eq!(echoed, b"hello conduit");

        let ca = test_ca();
        assert!(!ca.cert_der.is_empty());
        assert!(ca.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(ca.key_pem.contains("BEGIN PRIVATE KEY"));

        let origin = spawn_origin().await;
        let origin_ca = TestCa {
            cert_pem: String::new(),
            key_pem: String::new(),
            cert_der: origin.ca_der.to_vec(),
        };
        let tcp = TcpStream::connect(origin.addr).await.unwrap();
        let mut tls = tls_client(tcp, &origin_ca, "localhost", &[b"http/1.1"])
            .await
            .unwrap();
        tls.write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut resp = Vec::new();
        tls.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8(resp).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "got: {text:?}");
        assert!(text.contains("hit 1 GET /ping 0"), "got: {text:?}");
    }
}
