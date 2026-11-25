//! Per-profile / per-rule traffic counters (`hits`, `active`, `bytes_in`, `bytes_out`) are
//! updated end to end by the HTTP forward, HTTP `CONNECT`, and SOCKS engines, and agree with
//! the structured access records delivered to an `AccessLogger` sink.
//!
//! The fixture uses a `host_wildcard` rule, so it needs the default-on `routing` feature.

#![cfg(feature = "routing")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rusty_conduit_engine::logging::{
    AccessLogConfig, AccessLogger, AccessRecord, LogFormat, LogLevel, Logger,
};
use rusty_conduit_engine::router::DEFAULT_RULE_LABEL;
use rusty_conduit_engine::routing::{
    RoutingConfig, RoutingCounters, RoutingInput, RoutingState, TrafficSnapshot,
};
use rusty_conduit_engine::server::{EngineHandlers, ListenPlan, Server};
use rusty_conduit_engine::upstream::DialerOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const ROUTING: &str = r#"
default = "direct"

[[rules]]
name = "loopback"
cidr = "127.0.0.0/8"
target = "direct"

[[rules]]
name = "blocked"
host_wildcard = "*.blocked.example"
target = "block"
"#;

fn quiet() -> Logger {
    Logger::with_writer(LogLevel::Quiet, Box::new(std::io::sink()))
}

async fn echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

/// Answers every request with a 5-byte body and closes the connection.
async fn http_origin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .await;
                let _ = s.shutdown().await;
            });
        }
    });
    port
}

async fn socks5_connect(addr: SocketAddr, host: &str, port: u16) -> (TcpStream, u8) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut m = [0u8; 2];
    s.read_exact(&mut m).await.unwrap();
    assert_eq!(m, [5, 0]);
    let mut req = vec![5, 1, 0, 3, host.len() as u8];
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut head = [0u8; 10];
    s.read_exact(&mut head).await.unwrap();
    (s, head[1])
}

async fn echo_roundtrip(s: &mut TcpStream, payload: &[u8]) {
    s.write_all(payload).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    s.read_exact(&mut back).await.unwrap();
    assert_eq!(back, payload);
}

/// Close our side and wait for the proxy to close the relay.
async fn close(mut s: TcpStream) {
    s.shutdown().await.unwrap();
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest).await;
}

/// Wait until `f(counters)` holds (the engines finish records asynchronously).
async fn eventually(counters: &RoutingCounters, f: impl Fn(&RoutingCounters) -> bool) {
    for _ in 0..200 {
        if f(counters) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "counters never reached the expected state: profiles={:?} rules={:?}",
        counters.profiles(),
        counters.rules()
    );
}

fn snap(active: u64, bytes_in: u64, bytes_out: u64, hits: u64) -> TrafficSnapshot {
    TrafficSnapshot {
        active,
        bytes_in,
        bytes_out,
        hits,
    }
}

#[tokio::test]
async fn engines_update_profile_and_rule_counters() {
    let input = RoutingInput {
        routing: toml::from_str::<RoutingConfig>(ROUTING).unwrap(),
        ..RoutingInput::default()
    };
    input.validate().unwrap();
    let state = RoutingState::from_input(&input, DialerOptions::default())
        .unwrap()
        .with_connect_timeout(Duration::from_secs(3));
    let counters = state.counters().clone();
    let routing = state.shared();

    let logger = quiet();
    let (records_tx, mut records) = mpsc::unbounded_channel();
    let access = AccessLogger::start(
        AccessLogConfig::new(LogLevel::Quiet, LogFormat::Text, None, None),
        logger.clone(),
    )
    .await
    .unwrap()
    .with_channel(records_tx);
    let handlers = EngineHandlers::from_resolver(Arc::clone(&routing) as _, access, logger.clone());
    let server = Server::bind(
        ListenPlan::Separate {
            http: Some("127.0.0.1:0".parse().unwrap()),
            socks: Some("127.0.0.1:0".parse().unwrap()),
        },
        Arc::new(handlers),
        logger,
    )
    .await
    .unwrap();
    let (http, socks) = (
        server.local_http_addr().unwrap(),
        server.local_socks_addr().unwrap(),
    );
    let token = server.shutdown_token();
    let run = tokio::spawn(server.run());
    let echo = echo_server().await;
    let origin = http_origin().await;

    // 1. SOCKS5 session: active while open, bytes added when it finishes.
    let (mut s, rep) = socks5_connect(socks, "127.0.0.1", echo).await;
    assert_eq!(rep, 0x00);
    echo_roundtrip(&mut s, b"ping").await;
    assert_eq!(counters.rule("loopback"), Some(snap(1, 0, 0, 1)));
    assert_eq!(counters.profile("direct"), Some(snap(1, 0, 0, 1)));
    assert_eq!(
        routing.load().rule_traffic("loopback"),
        Some(snap(1, 0, 0, 1))
    );
    close(s).await;
    eventually(&counters, |c| c.rule("loopback") == Some(snap(0, 4, 4, 1))).await;
    assert_eq!(counters.profile("direct"), Some(snap(0, 4, 4, 1)));
    match records.recv().await.unwrap() {
        AccessRecord::Socks(r) => {
            assert_eq!(
                (r.rule.as_str(), r.bytes_in, r.bytes_out),
                ("loopback", 4, 4)
            );
        }
        other => panic!("unexpected record {other:?}"),
    }

    // 2. HTTP CONNECT tunnel.
    let mut t = TcpStream::connect(http).await.unwrap();
    let authority = format!("127.0.0.1:{echo}");
    t.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        t.read_exact(&mut b).await.unwrap();
        head.push(b[0]);
    }
    assert!(String::from_utf8_lossy(&head).contains(" 200"));
    echo_roundtrip(&mut t, b"hello!").await;
    assert_eq!(counters.rule("loopback"), Some(snap(1, 4, 4, 2)));
    close(t).await;
    eventually(&counters, |c| {
        c.rule("loopback") == Some(snap(0, 10, 10, 2))
    })
    .await;

    // 3. Plain HTTP forward: the 5-byte response body is `bytes_out`.
    let mut f = TcpStream::connect(http).await.unwrap();
    f.write_all(
        format!(
            "GET http://127.0.0.1:{origin}/ HTTP/1.1\r\nHost: 127.0.0.1:{origin}\r\n\
             Connection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    f.read_to_end(&mut resp).await.unwrap();
    assert!(
        resp.ends_with(b"hello"),
        "{}",
        String::from_utf8_lossy(&resp)
    );
    eventually(&counters, |c| {
        c.rule("loopback") == Some(snap(0, 10, 15, 3))
    })
    .await;

    // 4. Blocked route: counted under the rule and the `block` profile, no bytes.
    let (_b, rep) = socks5_connect(socks, "db.blocked.example", 5432).await;
    assert_eq!(rep, 0x02);
    eventually(&counters, |c| c.rule("blocked") == Some(snap(0, 0, 0, 1))).await;
    assert_eq!(counters.profile("block"), Some(snap(0, 0, 0, 1)));

    // 5. Table default.
    let (mut d, rep) = socks5_connect(socks, "localhost", echo).await;
    assert_eq!(rep, 0x00);
    echo_roundtrip(&mut d, b"abc").await;
    close(d).await;
    eventually(&counters, |c| {
        c.rule(DEFAULT_RULE_LABEL) == Some(snap(0, 3, 3, 1))
    })
    .await;
    assert_eq!(counters.profile("direct"), Some(snap(0, 13, 18, 4)));
    assert_eq!(
        counters.rules().keys().collect::<Vec<_>>(),
        [DEFAULT_RULE_LABEL, "blocked", "loopback"]
    );

    // Counters carried into a reloaded snapshot keep accumulating.
    let reloaded = RoutingState::from_input(&input, DialerOptions::default())
        .unwrap()
        .with_counters(counters.clone())
        .with_generation(1);
    assert_eq!(reloaded.counters(), &counters);
    routing.store(Arc::new(reloaded));
    let (mut s, rep) = socks5_connect(socks, "127.0.0.1", echo).await;
    assert_eq!(rep, 0x00);
    echo_roundtrip(&mut s, b"x").await;
    close(s).await;
    eventually(&counters, |c| {
        c.rule("loopback") == Some(snap(0, 11, 16, 4))
    })
    .await;

    token.cancel();
    run.await.unwrap();
}
