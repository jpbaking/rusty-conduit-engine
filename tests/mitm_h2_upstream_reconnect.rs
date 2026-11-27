//! MITM over HTTP/2 when the origin's connection goes away under a client connection that
//! stays open: the proxy redials the origin instead of answering `502`, resends a request
//! only when that is safe, and asks the client to reconnect (GOAWAY) if the origin stops
//! negotiating `h2`. `te: trailers` is forwarded end to end.

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode, Version};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{RootCertStore, ServerConfig};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_util::sync::CancellationToken;

use rusty_conduit_engine::http::mitm_tunnel::client_config_with_roots;
use rusty_conduit_engine::logging::{AccessLogConfig, AccessLogger, LogFormat, LogLevel, Logger};
use rusty_conduit_engine::mitm::ca::CaMaterial;
use rusty_conduit_engine::mitm::signer::LeafSigner;
use rusty_conduit_engine::mitm::{crypto_provider, MitmOptions, MitmState};
use rusty_conduit_engine::routing::{RoutingInput, RoutingState};
use rusty_conduit_engine::server::{EngineHandlers, ListenPlan, Server, ShutdownReport};
use rusty_conduit_engine::upstream::DialerOptions;

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

async fn read_head<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> String {
    let mut buf = Vec::new();
    while !buf.ends_with(b"\r\n\r\n") {
        match s.read_u8().await {
            Ok(b) => buf.push(b),
            Err(_) => break,
        }
    }
    String::from_utf8(buf).unwrap()
}

/// How the test origin treats each connection.
#[derive(Clone, Copy)]
enum Origin {
    /// HTTP/2: answer one request, then GOAWAY and close.
    GoawayAfterOne,
    /// HTTP/2: answer the first request, then drop the connection, without answering,
    /// when the second one arrives.
    DropOnSecond,
    /// HTTP/2 with GOAWAY after one request on the first connection; HTTP/1.1 only after.
    H2ThenH1,
}

/// What the origin saw: connections accepted and requests received (answered or not).
#[derive(Default)]
struct Seen {
    conns: AtomicUsize,
    requests: AtomicUsize,
}

/// Every answer is `hello`, with `x-te` echoing the request's `TE` header (or `none`) and
/// `x-proto` naming the HTTP version the origin served.
fn answer(req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let te = req
        .headers()
        .get("te")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none")
        .to_string();
    let proto = if req.version() == Version::HTTP_2 {
        "h2"
    } else {
        "http/1.1"
    };
    let cookie_len = req.headers().get("cookie").map_or(0, |v| v.len());
    let mut resp = Response::builder()
        .header("x-te", te)
        .header("x-proto", proto)
        .header("x-cookie-len", cookie_len.to_string());
    if req.uri().path() == "/big" {
        resp = resp.header("x-big", "b".repeat(BIG));
    }
    resp.body(Full::new(Bytes::from_static(b"hello"))).unwrap()
}

/// Header size past hyper's 16 KiB HTTP/2 default (real sites send ~26 KiB of headers).
const BIG: usize = 30 * 1024;

fn tls_acceptor(leaf: &Arc<CertifiedKey>, alpn: &[u8]) -> TlsAcceptor {
    let mut cfg = ServerConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedCert(Arc::clone(leaf))));
    cfg.alpn_protocols = vec![alpn.to_vec()];
    TlsAcceptor::from(Arc::new(cfg))
}

async fn serve_h2_conn(
    tls: tokio_rustls::server::TlsStream<TcpStream>,
    mode: Origin,
    seen: Arc<Seen>,
) {
    let answered = Arc::new(Notify::new());
    let dropping = Arc::new(Notify::new());
    let per_conn = Arc::new(AtomicUsize::new(0));
    let (a, d) = (Arc::clone(&answered), Arc::clone(&dropping));
    let service = hyper::service::service_fn(move |req: Request<Incoming>| {
        let (a, d, seen, per_conn) = (
            Arc::clone(&a),
            Arc::clone(&d),
            Arc::clone(&seen),
            Arc::clone(&per_conn),
        );
        async move {
            seen.requests.fetch_add(1, Ordering::SeqCst);
            let nth = per_conn.fetch_add(1, Ordering::SeqCst);
            if matches!(mode, Origin::DropOnSecond) && nth >= 1 {
                d.notify_one();
                std::future::pending::<()>().await;
            }
            let resp = answer(&req);
            let _ = req.into_body().collect().await;
            a.notify_one();
            Ok::<_, Infallible>(resp)
        }
    });
    let conn = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
        .max_header_list_size(256 * 1024)
        .serve_connection(TokioIo::new(tls), service);
    let mut conn = std::pin::pin!(conn);
    match mode {
        Origin::GoawayAfterOne | Origin::H2ThenH1 => {
            tokio::select! {
                _ = conn.as_mut() => {}
                () = answered.notified() => {
                    // Let the response go out, then GOAWAY and close.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    conn.as_mut().graceful_shutdown();
                    let _ = conn.as_mut().await;
                }
            }
        }
        Origin::DropOnSecond => {
            tokio::select! {
                _ = conn.as_mut() => {}
                // Drop the connection abruptly: no GOAWAY, no answer.
                () = dropping.notified() => {}
            }
        }
    }
}

/// HTTPS origin for `localhost` behaving as `mode`. Returns its port, CA, and counters.
async fn spawn_origin(mode: Origin) -> (u16, CertificateDer<'static>, Arc<Seen>) {
    let (cert_pem, key_pem, ca_der) = test_ca("Engine H2 Origin CA");
    let ca = CaMaterial::from_pem_bytes(cert_pem.as_bytes(), key_pem.as_bytes(), None).unwrap();
    let leaf = LeafSigner::new(&ca, Duration::from_secs(3600), crypto_provider())
        .unwrap()
        .sign_at("localhost", OffsetDateTime::now_utc())
        .unwrap()
        .certified;
    let h2 = tls_acceptor(&leaf, b"h2");
    let h1 = tls_acceptor(&leaf, b"http/1.1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Seen::default());
    let counters = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let nth = counters.conns.fetch_add(1, Ordering::SeqCst);
            let seen = Arc::clone(&counters);
            let as_h1 = matches!(mode, Origin::H2ThenH1) && nth >= 1;
            let acceptor = if as_h1 { h1.clone() } else { h2.clone() };
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                if as_h1 {
                    let service = hyper::service::service_fn(move |req: Request<Incoming>| {
                        let seen = Arc::clone(&seen);
                        async move {
                            seen.requests.fetch_add(1, Ordering::SeqCst);
                            Ok::<_, Infallible>(answer(&req))
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tls), service)
                        .await;
                } else {
                    serve_h2_conn(tls, mode, seen).await;
                }
            });
        }
    });
    (port, CertificateDer::from(ca_der), seen)
}

/// A MITM proxy trusting the origin CA, and one HTTP/2 client connection through it to
/// `localhost:origin_port`. Returns the sender, the request URL, and the proxy's
/// shutdown token and task.
async fn h2_client_via_mitm(
    origin_port: u16,
    origin_ca: CertificateDer<'static>,
) -> (
    hyper::client::conn::http2::SendRequest<Full<Bytes>>,
    String,
    CancellationToken,
    tokio::task::JoinHandle<ShutdownReport>,
) {
    let (mitm_cert, mitm_key, mitm_ca_der) = test_ca("Engine MITM CA");
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
    let state = RoutingState::from_input(&RoutingInput::default(), DialerOptions::default())
        .unwrap()
        .with_connect_timeout(Duration::from_secs(3));
    let logger = Logger::with_writer(LogLevel::Quiet, Box::new(std::io::sink()));
    let access = AccessLogger::start(
        AccessLogConfig::new(LogLevel::Quiet, LogFormat::Text, None, None),
        logger.clone(),
    )
    .await
    .unwrap();
    let handlers = EngineHandlers::from_resolver(state.shared(), access, logger.clone())
        .with_mitm(Arc::new(mitm.interceptor), false);
    let server = Server::bind(
        ListenPlan::Separate {
            http: Some("127.0.0.1:0".parse().unwrap()),
            socks: None,
        },
        Arc::new(handlers),
        logger,
    )
    .await
    .unwrap();
    let proxy = server.local_http_addr().unwrap();
    let token = server.shutdown_token();
    let run = tokio::spawn(server.run());

    let authority = format!("localhost:{origin_port}");
    let mut tcp = TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert!(read_head(&mut tcp).await.contains(" 200"));
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(mitm_ca_der)).unwrap();
    let mut client_cfg = client_config_with_roots(crypto_provider(), roots).unwrap();
    client_cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let tls = TlsConnector::from(Arc::new(client_cfg))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
    let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .max_header_list_size(256 * 1024)
        .handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    (sender, format!("https://{authority}/"), token, run)
}

/// `(status, x-te, x-proto, body)` of one request on `sender`.
async fn send(
    sender: &mut hyper::client::conn::http2::SendRequest<Full<Bytes>>,
    req: Request<Full<Bytes>>,
) -> (StatusCode, String, String, Bytes) {
    sender
        .ready()
        .await
        .expect("client HTTP/2 connection to the proxy closed");
    let resp = sender.send_request(req).await.unwrap();
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let (te, proto) = (header("x-te"), header("x-proto"));
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, te, proto, body)
}

fn get(url: &str) -> Request<Full<Bytes>> {
    Request::get(url).body(Full::new(Bytes::new())).unwrap()
}

#[tokio::test]
async fn h2_redials_after_origin_goaway() {
    let (port, ca, seen) = spawn_origin(Origin::GoawayAfterOne).await;
    let (mut sender, url, token, run) = h2_client_via_mitm(port, ca).await;
    for round in 1..=3 {
        let (status, _, proto, body) = send(&mut sender, get(&url)).await;
        assert_eq!(status, StatusCode::OK, "round {round}: {body:?}");
        assert_eq!((proto.as_str(), &body[..]), ("h2", &b"hello"[..]));
        // Give the origin time to send GOAWAY and close its side.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(seen.conns.load(Ordering::SeqCst), 3);
    token.cancel();
    run.await.unwrap();
}

#[tokio::test]
async fn h2_forwards_te_trailers() {
    let (port, ca, _) = spawn_origin(Origin::GoawayAfterOne).await;
    let (mut sender, url, token, run) = h2_client_via_mitm(port, ca).await;
    let mut req = get(&url);
    req.headers_mut().insert("te", "trailers".parse().unwrap());
    let (status, te, _, _) = send(&mut sender, req).await;
    assert_eq!((status, te.as_str()), (StatusCode::OK, "trailers"));
    token.cancel();
    run.await.unwrap();
}

#[tokio::test]
async fn h2_resends_get_dropped_by_origin() {
    let (port, ca, seen) = spawn_origin(Origin::DropOnSecond).await;
    let (mut sender, url, token, run) = h2_client_via_mitm(port, ca).await;
    let (status, ..) = send(&mut sender, get(&url)).await;
    assert_eq!(status, StatusCode::OK);
    // The origin drops the connection on this one; the proxy resends it on a new one.
    let (status, _, _, body) = send(&mut sender, get(&url)).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(&body[..], b"hello");
    assert_eq!(seen.conns.load(Ordering::SeqCst), 2);
    assert_eq!(seen.requests.load(Ordering::SeqCst), 3);
    token.cancel();
    run.await.unwrap();
}

#[tokio::test]
async fn h2_never_resends_post_dropped_by_origin() {
    let (port, ca, seen) = spawn_origin(Origin::DropOnSecond).await;
    let (mut sender, url, token, run) = h2_client_via_mitm(port, ca).await;
    let (status, ..) = send(&mut sender, get(&url)).await;
    assert_eq!(status, StatusCode::OK);
    let post = Request::builder()
        .method(Method::POST)
        .uri(&url)
        .body(Full::new(Bytes::from_static(b"order=1")))
        .unwrap();
    let (status, ..) = send(&mut sender, post).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    // Received once and never replayed.
    assert_eq!(seen.requests.load(Ordering::SeqCst), 2);
    assert_eq!(seen.conns.load(Ordering::SeqCst), 1);
    // The tunnel still works: the next request redials.
    let (status, ..) = send(&mut sender, get(&url)).await;
    assert_eq!(status, StatusCode::OK);
    token.cancel();
    run.await.unwrap();
}

#[tokio::test]
async fn h2_origin_turned_http1_is_served_then_client_gets_goaway() {
    let (port, ca, _) = spawn_origin(Origin::H2ThenH1).await;
    let (mut sender, url, token, run) = h2_client_via_mitm(port, ca).await;
    let (status, _, proto, _) = send(&mut sender, get(&url)).await;
    assert_eq!((status, proto.as_str()), (StatusCode::OK, "h2"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The redial negotiates HTTP/1.1: this request is still answered, over HTTP/1.1.
    let mut req = get(&url);
    req.headers_mut().insert("te", "trailers".parse().unwrap());
    let (status, te, proto, body) = send(&mut sender, req).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!((proto.as_str(), te.as_str()), ("http/1.1", "none"));
    // ...and the client connection is shut down so the client reconnects.
    for _ in 0..200 {
        if sender.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(sender.is_closed(), "client connection was not sent GOAWAY");
    token.cancel();
    run.await.unwrap();
}

#[tokio::test]
async fn h2_relays_headers_larger_than_16k() {
    let (port, ca, _) = spawn_origin(Origin::GoawayAfterOne).await;
    let (mut sender, url, token, run) = h2_client_via_mitm(port, ca).await;
    // A 30 KiB response header (like a long content-security-policy).
    let resp = {
        sender.ready().await.unwrap();
        sender
            .send_request(get(&format!("{url}big")))
            .await
            .unwrap()
    };
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-big").map(|v| v.len()), Some(BIG));
    let _ = resp.into_body().collect().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A 30 KiB request cookie.
    let mut req = get(&url);
    req.headers_mut()
        .insert("cookie", format!("c={}", "c".repeat(BIG)).parse().unwrap());
    sender.ready().await.unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let len = resp
        .headers()
        .get("x-cookie-len")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(len, (BIG + 2).to_string());
    token.cancel();
    run.await.unwrap();
}
