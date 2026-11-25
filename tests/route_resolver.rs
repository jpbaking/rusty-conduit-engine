//! A custom [`RouteResolver`] plugged into the HTTP and SOCKS engines through
//! `EngineHandlers::from_resolver` overrides routing decisions end to end.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rusty_conduit_engine::filter::AccessDenied;
use rusty_conduit_engine::logging::{AccessLogConfig, AccessLogger, LogFormat, LogLevel, Logger};
use rusty_conduit_engine::router::{RouteDecision, RouteRequest};
use rusty_conduit_engine::routing::{ProfileType, RouteResolver, RoutingInput, RoutingState};
use rusty_conduit_engine::server::{EngineHandlers, ListenPlan, Server};
use rusty_conduit_engine::upstream::DialerOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Routes `*.blocked.example` to the built-in `block` profile, denies `denied.example`, and
/// otherwise defers to the wrapped snapshot (default `direct`). Counts every call.
#[derive(Debug, Clone)]
struct MockResolver {
    inner: RoutingState,
    calls: Arc<AtomicUsize>,
}

impl RouteResolver for MockResolver {
    fn resolve(&self, req: &RouteRequest<'_>) -> Result<RouteDecision, AccessDenied> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if req.host == "denied.example" {
            return Err(AccessDenied("mock resolver denied".into()));
        }
        let mut d = self.inner.router.route(req)?;
        if req.host.ends_with(".blocked.example") {
            let t = &mut d.targets[0];
            t.name = "block".into();
            t.profile = "block".into();
            t.kind = ProfileType::Block;
            d.targets.truncate(1);
            d.rule = Some("mock".into());
        }
        Ok(d)
    }

    fn state(&self) -> Arc<RoutingState> {
        Arc::new(self.inner.clone())
    }

    fn pin(&self) -> Arc<dyn RouteResolver> {
        Arc::new(self.clone())
    }
}

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

async fn socks5_connect(addr: std::net::SocketAddr, host: &str, port: u16) -> (TcpStream, u8) {
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

async fn http_connect_status(addr: std::net::SocketAddr, authority: &str) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = vec![0u8; 256];
    let n = s.read(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn custom_route_resolver_drives_http_and_socks_engines() {
    let inner = RoutingState::from_input(&RoutingInput::default(), DialerOptions::default())
        .unwrap()
        .with_connect_timeout(Duration::from_secs(3));
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver = Arc::new(MockResolver {
        inner,
        calls: Arc::clone(&calls),
    });
    let logger = quiet();
    let access = AccessLogger::start(
        AccessLogConfig::new(LogLevel::Quiet, LogFormat::Text, None, None),
        logger.clone(),
    )
    .await
    .unwrap();
    let handlers = EngineHandlers::from_resolver(resolver, access, logger.clone());
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

    // SOCKS5: the mock's `block` decision is answered with 0x02, its denial too.
    let (_, rep) = socks5_connect(socks, "db.blocked.example", 5432).await;
    assert_eq!(rep, 0x02);
    let (_, rep) = socks5_connect(socks, "denied.example", 80).await;
    assert_eq!(rep, 0x02);
    // Deferred decisions dial `direct` through the resolver's snapshot.
    let (mut s, rep) = socks5_connect(socks, "localhost", echo).await;
    assert_eq!(rep, 0x00);
    s.write_all(b"ping").await.unwrap();
    let mut b = [0u8; 4];
    s.read_exact(&mut b).await.unwrap();
    assert_eq!(&b, b"ping");

    // HTTP CONNECT: blocked and denied are 403, deferred is 200.
    assert!(http_connect_status(http, "www.blocked.example:443")
        .await
        .contains(" 403"));
    assert!(http_connect_status(http, "denied.example:443")
        .await
        .contains(" 403"));
    assert!(http_connect_status(http, &format!("127.0.0.1:{echo}"))
        .await
        .contains(" 200"));

    assert_eq!(calls.load(Ordering::SeqCst), 6);
    token.cancel();
    run.await.unwrap();
}

#[test]
fn routing_state_and_shared_routing_are_route_resolvers() {
    let state =
        RoutingState::from_input(&RoutingInput::default(), DialerOptions::default()).unwrap();
    let req = RouteRequest::new(
        rusty_conduit_engine::routing::Inbound::Socks,
        "a.example",
        80,
    );
    let fixed: Arc<dyn RouteResolver> = Arc::new(state.clone().with_generation(1));
    assert_eq!(fixed.resolve(&req).unwrap().targets[0].profile, "direct");
    assert_eq!(fixed.pin().state().config_generation, 1);

    let shared = state.with_generation(1).shared();
    let live: Arc<dyn RouteResolver> = shared.clone();
    let pinned = live.pin();
    shared.store(Arc::new(
        RoutingState::from_input(&RoutingInput::default(), DialerOptions::default())
            .unwrap()
            .with_generation(2),
    ));
    // A pinned view keeps its snapshot across a reload; the live handle follows it.
    assert_eq!(pinned.state().config_generation, 1);
    assert_eq!(live.state().config_generation, 2);
    assert_eq!(RouteResolver::state(&shared).config_generation, 2);
}
