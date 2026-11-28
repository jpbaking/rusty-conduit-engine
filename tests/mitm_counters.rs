//! MITM built from in-memory PEM CA bytes (`MitmState::from_pem_bytes_with_upstream`)
//! intercepts a `CONNECT`, and every decrypted request is counted in the routing state's
//! per-rule / per-profile traffic counters.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{RootCertStore, ServerConfig};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use rusty_conduit_engine::http::mitm_tunnel::client_config_with_roots;
use rusty_conduit_engine::logging::{
    AccessLogConfig, AccessLogger, AccessRecord, LogFormat, LogLevel, Logger,
};
use rusty_conduit_engine::mitm::ca::CaMaterial;
use rusty_conduit_engine::mitm::signer::LeafSigner;
use rusty_conduit_engine::mitm::{crypto_provider, MitmOptions, MitmState};
use rusty_conduit_engine::router::DEFAULT_RULE_LABEL;
use rusty_conduit_engine::routing::{
    OneOrMany, RoutingInput, RoutingState, RuleConfig, TrafficSnapshot,
};
use rusty_conduit_engine::server::{EngineHandlers, ListenPlan, Server};
use rusty_conduit_engine::upstream::DialerOptions;

/// `(cert PEM, key PEM, cert DER)` of an ephemeral CA.
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

/// HTTPS/1.1 origin for `localhost` answering every request with a 5-byte body.
async fn spawn_origin() -> (u16, CertificateDer<'static>) {
    let (cert_pem, key_pem, ca_der) = test_ca("Engine Origin CA");
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
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                read_head(&mut tls).await;
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (port, CertificateDer::from(ca_der))
}

#[tokio::test]
async fn mitm_from_pem_bytes_counts_decrypted_requests() {
    let (origin_port, origin_ca) = spawn_origin().await;
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
    assert!(mitm.ca_subject.contains("Engine MITM CA"));

    let state = RoutingState::from_input(&RoutingInput::default(), DialerOptions::default())
        .unwrap()
        .with_connect_timeout(Duration::from_secs(3));
    let counters = state.counters().clone();
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

    // CONNECT, then TLS to the MITM leaf (trusting only the in-memory MITM CA).
    let authority = format!("localhost:{origin_port}");
    let mut tcp = TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert!(read_head(&mut tcp).await.contains(" 200"));
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(mitm_ca_der)).unwrap();
    let mut client_cfg = client_config_with_roots(crypto_provider(), roots).unwrap();
    client_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let mut tls = TlsConnector::from(Arc::new(client_cfg))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    tls.write_all(
        format!("GET /counted HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    let _ = tls.read_to_end(&mut resp).await;
    assert!(
        resp.ends_with(b"hello"),
        "{}",
        String::from_utf8_lossy(&resp)
    );
    drop(tls);

    let expected = TrafficSnapshot {
        active: 0,
        bytes_in: 0,
        bytes_out: 5,
        hits: 1,
    };
    for _ in 0..200 {
        if counters.rule(DEFAULT_RULE_LABEL) == Some(expected) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(counters.rule(DEFAULT_RULE_LABEL), Some(expected));
    assert_eq!(counters.profile("direct"), Some(expected));

    token.cancel();
    run.await.unwrap();
}

#[tokio::test]
async fn bypass_mitm_rule_tunnels_opaquely_while_mitm_is_enabled() {
    let (origin_port, origin_ca) = spawn_origin().await;
    let (mitm_cert, mitm_key, _mitm_ca_der) = test_ca("Engine MITM CA");

    let mut origin_roots = RootCertStore::empty();
    origin_roots.add(origin_ca.clone()).unwrap();
    let upstream = client_config_with_roots(crypto_provider(), origin_roots.clone()).unwrap();
    let mitm = MitmState::from_pem_bytes_with_upstream(
        mitm_cert.as_bytes(),
        mitm_key.as_bytes(),
        &MitmOptions::default(),
        crypto_provider(),
        upstream,
    )
    .unwrap();

    let mut input = RoutingInput {
        mitm_enabled: true,
        ..RoutingInput::default()
    };
    input.routing.rules.push(RuleConfig {
        name: Some("bypass-localhost".to_string()),
        inbound: None,
        host: Some(OneOrMany::One("localhost".to_string())),
        host_wildcard: None,
        host_regex: None,
        cidr: None,
        ip: None,
        port: None,
        host_levels: None,
        bypass: None,
        weekday: None,
        time: None,
        url_wildcard: None,
        url_regex: None,
        keyword: None,
        match_all: false,
        disabled: false,
        resolve_dns: None,
        bypass_mitm: true,
        target: OneOrMany::One("direct".to_string()),
    });

    let state = RoutingState::from_input(&input, DialerOptions::default())
        .unwrap()
        .with_connect_timeout(Duration::from_secs(3));
    let counters = state.counters().clone();
    let records: Arc<Mutex<Vec<AccessRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    let logger = Logger::with_writer(LogLevel::Quiet, Box::new(std::io::sink()));
    let access = AccessLogger::start(
        AccessLogConfig::new(LogLevel::Info, LogFormat::Text, None, None),
        logger.clone(),
    )
    .await
    .unwrap()
    .with_sink(Arc::new(move |rec| {
        sink_records.lock().unwrap().push(rec.clone());
    }));
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

    // Client trusts ONLY the origin CA (not the MITM CA): handshake succeeds only if MITM is bypassed.
    let authority = format!("localhost:{origin_port}");
    let mut tcp = TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert!(read_head(&mut tcp).await.contains(" 200"));
    let mut client_cfg = client_config_with_roots(crypto_provider(), origin_roots).unwrap();
    client_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let mut tls = TlsConnector::from(Arc::new(client_cfg))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    tls.write_all(
        format!("GET /bypass HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    let _ = tls.read_to_end(&mut resp).await;
    assert!(
        resp.ends_with(b"hello"),
        "{}",
        String::from_utf8_lossy(&resp)
    );
    drop(tls);

    for _ in 0..200 {
        if counters
            .rule("bypass-localhost")
            .is_some_and(|s| s.hits == 1 && s.active == 0)
            && !records.lock().unwrap().is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let snap = counters.rule("bypass-localhost").expect("rule counter");
    assert_eq!(snap.hits, 1);
    assert_eq!(snap.active, 0);
    assert!(snap.bytes_out > 0);

    {
        let recs = records.lock().unwrap();
        assert!(
            recs.iter().any(|r| matches!(
                r,
                AccessRecord::Http(h) if !h.mitm && h.method == "CONNECT" && h.rule == "bypass-localhost"
            )),
            "expected non-MITM CONNECT access record for bypass-localhost, got {recs:?}"
        );
    }

    token.cancel();
    run.await.unwrap();
}

#[tokio::test]
async fn mitm_bypass_hosts_tunnels_opaquely_without_routing_rule() {
    let (origin_port, origin_ca) = spawn_origin().await;
    let (mitm_cert, mitm_key, _mitm_ca_der) = test_ca("Engine MITM CA");

    let mut origin_roots = RootCertStore::empty();
    origin_roots.add(origin_ca.clone()).unwrap();
    let upstream = client_config_with_roots(crypto_provider(), origin_roots.clone()).unwrap();
    let opts = MitmOptions {
        bypass_hosts: vec!["localhost".parse().unwrap()],
        ..MitmOptions::default()
    };
    let mitm = MitmState::from_pem_bytes_with_upstream(
        mitm_cert.as_bytes(),
        mitm_key.as_bytes(),
        &opts,
        crypto_provider(),
        upstream,
    )
    .unwrap();

    let state = RoutingState::from_input(&RoutingInput::default(), DialerOptions::default())
        .unwrap()
        .with_connect_timeout(Duration::from_secs(3));
    let records: Arc<Mutex<Vec<AccessRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    let logger = Logger::with_writer(LogLevel::Quiet, Box::new(std::io::sink()));
    let access = AccessLogger::start(
        AccessLogConfig::new(LogLevel::Info, LogFormat::Text, None, None),
        logger.clone(),
    )
    .await
    .unwrap()
    .with_sink(Arc::new(move |rec| {
        sink_records.lock().unwrap().push(rec.clone());
    }));
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
    let mut client_cfg = client_config_with_roots(crypto_provider(), origin_roots).unwrap();
    client_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let mut tls = TlsConnector::from(Arc::new(client_cfg))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    tls.write_all(
        format!("GET /bypass-host HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    let _ = tls.read_to_end(&mut resp).await;
    assert!(
        resp.ends_with(b"hello"),
        "{}",
        String::from_utf8_lossy(&resp)
    );
    drop(tls);

    for _ in 0..200 {
        if !records.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    {
        let recs = records.lock().unwrap();
        assert!(
            recs.iter().any(|r| matches!(
                r,
                AccessRecord::Http(h) if !h.mitm && h.method == "CONNECT"
            )),
            "expected non-MITM CONNECT access record, got {recs:?}"
        );
    }

    token.cancel();
    run.await.unwrap();
}

#[test]
fn rule_config_parses_bypass_mitm_in_toml() {
    let default_rule: RuleConfig = toml::from_str(
        r#"
        host = "example.com"
        target = "direct"
        "#,
    )
    .unwrap();
    assert!(!default_rule.bypass_mitm);

    let bypass_rule: RuleConfig = toml::from_str(
        r#"
        host = "bank.example.com"
        bypass_mitm = true
        target = "direct"
        "#,
    )
    .unwrap();
    assert!(bypass_rule.bypass_mitm);
}
