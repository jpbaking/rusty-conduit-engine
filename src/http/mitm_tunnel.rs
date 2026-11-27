//! HTTPS `CONNECT` interception (MITM ON) over Auto Switch routed upstreams.
//!
//! After the client received `200 Connection Established` ([`super::RequestCtx`] hands the
//! upgraded stream to [`MitmInterceptor`]):
//!
//! 1. `tokio_rustls::LazyConfigAcceptor` reads the client's `ClientHello` (SNI + ALPN).
//!    The leaf host is the SNI, falling back to the `CONNECT` host/IP.
//! 2. Upstream connections are dialed through the routing snapshot taken at `CONNECT` time:
//!    [`Router`](crate::router::Router) decision → failover chain from the
//!    [`ProfileRegistry`](crate::upstream::ProfileRegistry) → an
//!    [`UpstreamStream`] (`direct`, `socks5`, `socks4a`, `http` / `https` `CONNECT`). Upstream
//!    TLS (`tokio-rustls` client, SNI = leaf host, verified per
//!    [`UpstreamTlsVerification`] — see [`upstream_client_config`] — or with an injected
//!    config) then runs **over that stream**, so TLS-over-TLS to an `https`
//!    proxy works the same way as a plain TCP tunnel.
//! 3. The leaf certificate comes from the [`CertCache`] (signed on a miss) and the client
//!    TLS handshake is completed.
//! 4. Decrypted requests are served with `hyper` and forwarded upstream, streaming bodies
//!    frame by frame — **nothing is cached**.
//!
//! Two routing modes:
//!
//! * **No URL-level rules** (`url_wildcard`, `url_regex`, `keyword`) in the tables active for
//!   HTTP: the route is resolved **once** at `CONNECT` time, the upstream is dialed before the
//!   client handshake (so the client sees exactly the ALPN protocol, `h2` or `http/1.1`, the
//!   origin chose), and that upstream TLS connection is reused for every request on the
//!   tunnel. An upstream connection that closes (HTTP/1.1 close, HTTP/2 GOAWAY, reset) is
//!   redialed through the same decision on the next request, so the client connection
//!   outlives it. Over HTTP/2, a request the origin dropped unanswered is resent once on
//!   the new connection if it was never written, or if it is idempotent and has no body;
//!   if a redial no longer negotiates `h2`, the requests in hand go out over HTTP/1.1 and
//!   the client is sent GOAWAY so it reconnects. `te: trailers` is kept on HTTP/2.
//! * **URL-level rules present**: every decrypted request is routed with its full
//!   `https://host/path?query` URL. The tunnel speaks HTTP/1.1 to the client and upstream
//!   connections are dialed lazily; a request whose decision resolves to a different
//!   failover chain than the current upstream connection dials a fresh connection through
//!   the new chain. A request routed to `block` gets `403 Forbidden`.
//!
//! HTTP/1.1 upgrades (`101 Switching Protocols`, e.g. WebSocket) are forwarded and then
//! relayed as raw bytes. One access record per decrypted request: `proto=https mitm=true`,
//! method, `https://authority/path`, `rule`, `profile`, redacted `upstream`, status, bytes,
//! duration. Upstream failures before any request are logged as one `method=CONNECT`
//! record.

use std::convert::Infallible;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty};
use hyper::body::{Body as _, Incoming};
use hyper::client::conn::{http1 as client_http1, http2 as client_http2};
use hyper::header::{self, HeaderValue};
use hyper::server::conn::{http1 as server_http1, http2 as server_http2};
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::{Request, Response, StatusCode, Uri, Version};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::{Acceptor, ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme};
use rustls_platform_verifier::BuilderVerifierExt;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use tokio_rustls::{client::TlsStream as ClientTlsStream, LazyConfigAcceptor, TlsConnector};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::error_page::page_target;
use super::forward::strip_hop_by_hop;
use super::tunnel::CountingStream;
use super::{
    dial_error_status, AccessGuard, ConnectTarget, CountingBody, ErrorPageTemplate, ProxyBody,
};
use crate::filter::FilterRule;
use crate::logging::{AccessLogger, HttpAccessRecord, Logger};
use crate::mitm::cache::CertCache;
use crate::mitm::UpstreamTlsVerification;
use crate::net::relay_bidirectional;
use crate::router::{RouteDecision, RouteRequest};
use crate::routing::Inbound;
use crate::routing::{RouteResolver, RoutingCounters};
use crate::server::RoutingState;
use crate::upstream::UpstreamStream;

/// Bound on the client and upstream TLS handshakes.
pub const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Largest HTTP/2 header block accepted from the client and from the origin (HPACK-decoded
/// size, `SETTINGS_MAX_HEADER_LIST_SIZE`). hyper's 16 KiB default is smaller than what
/// browsers accept (Chrome advertises 256 KiB), and real sites exceed it (long
/// `content-security-policy` and `set-cookie` headers), so a stricter proxy would fail
/// pages a browser loads directly.
pub const H2_MAX_HEADER_LIST_SIZE: u32 = 256 * 1024;

const ALPN_H2: &[u8] = b"h2";
const ALPN_HTTP11: &[u8] = b"http/1.1";

/// Upstream TLS session over a routed upstream stream.
type UpstreamTls = ClientTlsStream<UpstreamStream>;
type ClientTls = tokio_rustls::server::TlsStream<TokioIo<Upgraded>>;
type H1Sender = client_http1::SendRequest<ProxyBody>;
type H2Sender = client_http2::SendRequest<ProxyBody>;

/// Upstream TLS client config using the OS / platform certificate verifier.
pub fn platform_client_config(
    provider: Arc<CryptoProvider>,
) -> Result<ClientConfig, rustls::Error> {
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_platform_verifier()?
        .with_no_client_auth())
}

/// Upstream TLS client config trusting only `roots` (local integration tests, private PKI).
pub fn client_config_with_roots(
    provider: Arc<CryptoProvider>,
    roots: RootCertStore,
) -> Result<ClientConfig, rustls::Error> {
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

/// Upstream TLS client config for a [`UpstreamTlsVerification`] mode.
///
/// * `Strict` — [`platform_client_config`].
/// * `AllowInsecureHosts(rules)` — certificates of targets matching any rule (exact domain,
///   `*.domain`, IP, or CIDR) are accepted unverified; every other target is verified with
///   the platform verifier. An empty rule list is the same as `Strict`.
/// * `InsecureAll` — every upstream certificate is accepted unverified.
///
/// Handshake signatures are always verified against the certificate presented, so an
/// accepted certificate still has to belong to the peer completing the handshake.
pub fn upstream_client_config(
    provider: Arc<CryptoProvider>,
    verification: &UpstreamTlsVerification,
) -> Result<ClientConfig, rustls::Error> {
    let verifier: Arc<dyn ServerCertVerifier> = match verification {
        UpstreamTlsVerification::Strict => return platform_client_config(provider),
        UpstreamTlsVerification::AllowInsecureHosts(rules) if rules.is_empty() => {
            return platform_client_config(provider)
        }
        UpstreamTlsVerification::AllowInsecureHosts(rules) => {
            Arc::new(SelectiveServerCertVerifier {
                platform: Arc::new(rustls_platform_verifier::Verifier::new(Arc::clone(
                    &provider,
                ))?),
                rules: rules.clone(),
                provider: Arc::clone(&provider),
            })
        }
        UpstreamTlsVerification::InsecureAll => {
            Arc::new(InsecureServerCertVerifier(Arc::clone(&provider)))
        }
    };
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

fn verify_tls12(
    provider: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    verify_tls12_signature(
        message,
        cert,
        dss,
        &provider.signature_verification_algorithms,
    )
}

fn verify_tls13(
    provider: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    verify_tls13_signature(
        message,
        cert,
        dss,
        &provider.signature_verification_algorithms,
    )
}

/// `UpstreamTlsVerification::InsecureAll`: accepts any server certificate but still checks
/// handshake signatures.
#[derive(Debug)]
struct InsecureServerCertVerifier(Arc<CryptoProvider>);

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12(&self.0, message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13(&self.0, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// `UpstreamTlsVerification::AllowInsecureHosts`: skips certificate verification for
/// targets matching a rule and delegates every other target to the platform verifier.
#[derive(Debug)]
struct SelectiveServerCertVerifier {
    platform: Arc<rustls_platform_verifier::Verifier>,
    rules: Vec<FilterRule>,
    provider: Arc<CryptoProvider>,
}

impl SelectiveServerCertVerifier {
    fn is_insecure_target(&self, server_name: &ServerName<'_>) -> bool {
        match server_name {
            ServerName::DnsName(name) => {
                let domain = name.as_ref().trim_end_matches('.').to_ascii_lowercase();
                self.rules.iter().any(|r| r.matches_domain(&domain))
            }
            ServerName::IpAddress(ip) => {
                let ip = std::net::IpAddr::from(*ip);
                self.rules.iter().any(|r| r.matches_ip(ip))
            }
            _ => false,
        }
    }
}

impl ServerCertVerifier for SelectiveServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self.is_insecure_target(server_name) {
            return Ok(ServerCertVerified::assertion());
        }
        self.platform
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12(&self.provider, message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// An intercepted `CONNECT`, handed to [`MitmInterceptor::intercept`] after the client
/// received `200 Connection Established`.
pub(crate) struct MitmConnect {
    /// The upgraded client stream (raw bytes after the `CONNECT` response).
    pub(crate) upgraded: TokioIo<Upgraded>,
    pub(crate) target: ConnectTarget,
    pub(crate) client: SocketAddr,
    /// Cancelled on server shutdown.
    pub(crate) shutdown: CancellationToken,
    pub(crate) access: AccessLogger,
    pub(crate) logger: Logger,
    /// Routing snapshot of the client connection; used for the whole tunnel unless URL
    /// rules are configured.
    pub(crate) state: Arc<RoutingState>,
    /// Resolver pinned for the client connection (routes against [`Self::state`]).
    pub(crate) route: Arc<dyn RouteResolver>,
    /// The live resolver: with URL rules every decrypted request pins a fresh view of it
    /// (hot-reload).
    pub(crate) live: Arc<dyn RouteResolver>,
    /// The HTTP engine's error page; [`MitmInterceptor::with_error_page`] overrides it.
    pub(crate) error_page: ErrorPageTemplate,
}

/// The MITM `CONNECT` handler. Cheap to clone.
#[derive(Clone)]
pub struct MitmInterceptor {
    cache: Arc<CertCache>,
    upstream_tls: Arc<ClientConfig>,
    provider: Arc<CryptoProvider>,
    error_page: Option<ErrorPageTemplate>,
}

impl fmt::Debug for MitmInterceptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MitmInterceptor")
            .field("cache", &self.cache)
            .field("error_page", &self.error_page)
            .finish_non_exhaustive()
    }
}

/// Serves one fixed leaf certificate (chosen before the handshake from the `ClientHello`).
#[derive(Debug)]
struct FixedCert(Arc<CertifiedKey>);

impl ResolvesServerCert for FixedCert {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// `rule=` / `profile=` / `upstream=` of one upstream connection (or failed attempt).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RouteInfo {
    rule: String,
    profile: String,
    upstream: String,
    /// Where requests on this route are counted (`None` for filter denials), and the
    /// profile name they are counted under.
    counters: Option<RoutingCounters>,
    counted_profile: String,
}

/// A failed route / dial: the status to report and the route fields to log.
#[derive(Debug)]
struct DialFailure {
    status: StatusCode,
    info: RouteInfo,
}

/// Opens upstream TLS connections to the `CONNECT` target through routed upstream chains.
#[derive(Clone)]
struct Dialer {
    tls: Arc<ClientConfig>,
    target: ConnectTarget,
    server_name: ServerName<'static>,
    /// Pinned resolver deciding routes; [`Self::state`] is its dialing snapshot.
    route: Arc<dyn RouteResolver>,
    state: Arc<RoutingState>,
    logger: Logger,
}

/// Identity of a decision's failover chain: requests whose chains are equal share an
/// upstream connection.
fn chain_key(d: &RouteDecision) -> String {
    d.targets
        .iter()
        .map(|t| t.profile.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

impl Dialer {
    /// Route the target, optionally with the full decrypted request URL.
    fn route(&self, url: Option<&str>) -> Result<RouteDecision, DialFailure> {
        let req = RouteRequest {
            inbound: Inbound::Http,
            host: &self.target.host,
            port: self.target.port,
            scheme: Some("https"),
            url,
        };
        self.route.resolve(&req).map_err(|denied| {
            self.logger.info(format!(
                "MITM {} denied by filter: {denied}",
                url.unwrap_or(&self.target.authority())
            ));
            DialFailure {
                status: StatusCode::FORBIDDEN,
                info: RouteInfo::default(),
            }
        })
    }

    /// Dial `decision`'s failover chain to the target and run upstream TLS over the stream.
    async fn connect(
        &self,
        decision: &RouteDecision,
        alpn: Vec<Vec<u8>>,
    ) -> Result<(UpstreamTls, RouteInfo), DialFailure> {
        let rule = decision.rule_label().to_string();
        let failed = |status: StatusCode| {
            let (profile, upstream) = super::failed_route(&self.state, decision);
            DialFailure {
                status,
                info: RouteInfo {
                    rule: rule.clone(),
                    profile,
                    upstream,
                    counters: Some(self.state.counters.clone()),
                    counted_profile: RoutingState::counted_profile(decision, None),
                },
            }
        };
        let target = &self.target;
        let chain = match self.state.registry.chain_for(&decision.targets) {
            Ok(c) => c.with_logger(self.logger.clone()),
            Err(e) => {
                self.logger.error(format!("MITM {target}: {e}"));
                return Err(failed(StatusCode::BAD_GATEWAY));
            }
        };
        let outcome = match chain
            .dial(&target.host, target.port, self.state.connect_timeout)
            .await
        {
            Ok(o) => o,
            Err(e) => {
                let status = dial_error_status(&e);
                let f = failed(status);
                if status == StatusCode::FORBIDDEN {
                    self.logger.info(format!(
                        "MITM {target} blocked: rule={rule} profile={} upstream={}",
                        f.info.profile, f.info.upstream
                    ));
                } else {
                    self.logger.error(format!(
                        "MITM upstream {target} failed: rule={rule} profile={} upstream={} \
                         error={e}",
                        f.info.profile, f.info.upstream
                    ));
                }
                return Err(f);
            }
        };
        let profile = decision
            .targets
            .get(outcome.attempt)
            .map(|t| t.label())
            .unwrap_or_else(|| outcome.profile.clone());
        let info = RouteInfo {
            rule,
            profile,
            upstream: outcome.upstream,
            counters: Some(self.state.counters.clone()),
            counted_profile: RoutingState::counted_profile(decision, Some(outcome.attempt)),
        };
        let mut cfg = (*self.tls).clone();
        cfg.alpn_protocols = alpn;
        let connector = TlsConnector::from(Arc::new(cfg));
        let tls = tokio::time::timeout(
            TLS_HANDSHAKE_TIMEOUT,
            connector.connect(self.server_name.clone(), outcome.stream),
        )
        .await
        .unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "upstream TLS handshake timed out",
            ))
        });
        match tls {
            Ok(tls) => {
                self.logger.debug(format!(
                    "MITM upstream {target}: rule={} profile={} upstream={}",
                    info.rule, info.profile, info.upstream
                ));
                Ok((tls, info))
            }
            Err(e) => {
                self.logger.error(format!(
                    "MITM upstream TLS to {target} (SNI {:?}) via profile={} failed: {e}",
                    self.server_name, info.profile
                ));
                Err(DialFailure {
                    status: dial_error_status(&e),
                    info,
                })
            }
        }
    }
}

/// The upstream HTTP/1.1 connection currently attached to a tunnel.
struct H1Upstream {
    sender: H1Sender,
    key: String,
    info: RouteInfo,
}

/// The upstream side of an HTTP/2 tunnel: one multiplexed connection, redialed through
/// the tunnel's fixed decision when the origin closes it (GOAWAY, idle timeout, reset),
/// so the client's HTTP/2 connection outlives any one upstream connection.
struct H2Tunnel {
    decision: Arc<RouteDecision>,
    slot: Mutex<H2Slot>,
    /// Signalled when a redial no longer negotiates `h2`: the client connection is then
    /// shut down gracefully (GOAWAY) so the client reconnects with a fresh `CONNECT`.
    goaway: tokio::sync::Notify,
}

struct H2Slot {
    /// Bumped on every redial, so a request that saw one connection fail only retires that
    /// connection, never a newer one.
    generation: u64,
    upstream: H2Upstream,
}

enum H2Upstream {
    /// The current connection (it may since have closed: checked before each use).
    Open { sender: H2Sender, info: RouteInfo },
    /// Retired: the next request redials.
    Closed,
    /// A redial came back without `h2`. The client was sent GOAWAY; requests still
    /// arriving on it go out over one-off HTTP/1.1 connections.
    Downgraded,
}

/// The upstream a decrypted HTTP/2 request goes out on.
enum H2Pick {
    H2 {
        sender: H2Sender,
        info: RouteInfo,
        generation: u64,
    },
    /// A dedicated HTTP/1.1 connection (origin no longer negotiates `h2`).
    H1 {
        tls: Box<UpstreamTls>,
        info: RouteInfo,
    },
}

/// How a tunnel picks the upstream for each request.
#[derive(Clone)]
enum Routing {
    /// Resolved once at `CONNECT` time.
    Fixed {
        decision: Arc<RouteDecision>,
        info: RouteInfo,
    },
    /// Re-evaluated for every decrypted request (URL rules configured) against a fresh
    /// snapshot of the live runtime state.
    PerRequest(Arc<dyn RouteResolver>),
}

/// Per-tunnel state shared by every decrypted request.
#[derive(Clone)]
struct Session {
    access: AccessLogger,
    logger: Logger,
    tasks: TaskTracker,
    shutdown: CancellationToken,
    client: SocketAddr,
    /// Authority used when a request carries neither an absolute URI nor `Host`.
    default_authority: String,
    dialer: Dialer,
    routing: Routing,
    /// Template for errors the proxy answers decrypted requests with.
    error_page: ErrorPageTemplate,
}

impl MitmInterceptor {
    pub fn new(
        cache: Arc<CertCache>,
        upstream_tls: Arc<ClientConfig>,
        provider: Arc<CryptoProvider>,
    ) -> Self {
        Self {
            cache,
            upstream_tls,
            provider,
            error_page: None,
        }
    }

    /// Render errors for decrypted requests with `template`, overriding the HTTP engine's
    /// [`HttpProxy::with_error_page`](super::HttpProxy::with_error_page). Upstream
    /// responses are never rewritten.
    pub fn with_error_page(mut self, template: ErrorPageTemplate) -> Self {
        self.error_page = Some(template);
        self
    }

    pub fn cache(&self) -> &Arc<CertCache> {
        &self.cache
    }

    pub(crate) async fn intercept(&self, connect: MitmConnect) {
        let MitmConnect {
            upgraded,
            target,
            client,
            shutdown,
            access,
            logger,
            state,
            route,
            live,
            error_page,
        } = connect;
        let error_page = self.error_page.clone().unwrap_or(error_page);
        let tasks = TaskTracker::new();
        let connect_guard = |status: StatusCode, info: &RouteInfo| {
            let mut g = new_guard(&access, &tasks, client, "CONNECT", target.authority());
            set_route(&mut g, info);
            g.set_status(status);
            g
        };

        // 1. ClientHello: SNI + ALPN.
        let acceptor = LazyConfigAcceptor::new(Acceptor::default(), upgraded);
        let start = tokio::select! {
            () = shutdown.cancelled() => return,
            res = tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor) => match res {
                Ok(Ok(start)) => start,
                Ok(Err(e)) => {
                    logger.debug(format!("MITM {client} -> {target}: invalid ClientHello: {e}"));
                    return;
                }
                Err(_) => {
                    logger.debug(format!("MITM {client} -> {target}: ClientHello timed out"));
                    return;
                }
            },
        };
        let hello = start.client_hello();
        let sni = hello.server_name().map(str::to_string);
        let offered: Vec<Vec<u8>> = hello
            .alpn()
            .map(|it| {
                it.filter(|p| *p == ALPN_H2 || *p == ALPN_HTTP11)
                    .map(<[u8]>::to_vec)
                    .collect()
            })
            .unwrap_or_default();
        let offers_h11 = offered.iter().any(|p| p == ALPN_HTTP11);
        let leaf_host = sni.unwrap_or_else(|| target.host.clone());

        let Ok(server_name) = ServerName::try_from(leaf_host.clone()) else {
            logger.debug(format!(
                "MITM {client} -> {target}: invalid server name {leaf_host:?}"
            ));
            drop(connect_guard(
                StatusCode::BAD_REQUEST,
                &RouteInfo::default(),
            ));
            return;
        };
        let per_request = state.router.has_url_rules(Inbound::Http);
        let dialer = Dialer {
            tls: Arc::clone(&self.upstream_tls),
            target: target.clone(),
            server_name,
            route,
            state,
            logger: logger.clone(),
        };

        // 2. Upstream. Without URL rules: route once and dial now (learn the ALPN choice).
        let (routing, upstream, negotiated) = if per_request {
            let alpn = offers_h11.then(|| ALPN_HTTP11.to_vec());
            (Routing::PerRequest(live), None, alpn)
        } else {
            let dialed = async {
                let decision = dialer.route(None)?;
                let (tls, info) = dialer.connect(&decision, offered.clone()).await?;
                Ok::<_, DialFailure>((decision, tls, info))
            };
            let dialed = tokio::select! {
                () = shutdown.cancelled() => return,
                res = dialed => res,
            };
            match dialed {
                Ok((decision, tls, info)) => {
                    let negotiated = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                    let negotiated =
                        negotiated.or_else(|| offers_h11.then(|| ALPN_HTTP11.to_vec()));
                    let routing = Routing::Fixed {
                        decision: Arc::new(decision),
                        info,
                    };
                    (routing, Some(tls), negotiated)
                }
                Err(f) => {
                    drop(connect_guard(f.status, &f.info));
                    return;
                }
            }
        };

        // 3. Leaf certificate + client handshake.
        let leaf = match self.cache.get_or_create(&leaf_host).await {
            Ok(k) => k,
            Err(e) => {
                logger.error(format!("MITM {client} -> {target}: {e}"));
                drop(connect_guard(
                    StatusCode::BAD_GATEWAY,
                    &RouteInfo::default(),
                ));
                return;
            }
        };
        let mut server_cfg = match ServerConfig::builder_with_provider(Arc::clone(&self.provider))
            .with_safe_default_protocol_versions()
        {
            Ok(b) => b
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(FixedCert(leaf))),
            Err(e) => {
                logger.error(format!("MITM TLS server config failed: {e}"));
                return;
            }
        };
        server_cfg.alpn_protocols = negotiated.iter().cloned().collect();
        let client_tls = tokio::select! {
            () = shutdown.cancelled() => return,
            res = tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, start.into_stream(Arc::new(server_cfg))) => res,
        };
        let client_tls = match client_tls {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                logger.debug(format!(
                    "MITM client TLS handshake with {client} for {leaf_host} failed: {e}"
                ));
                return;
            }
            Err(_) => {
                logger.debug(format!("MITM client TLS handshake with {client} timed out"));
                return;
            }
        };

        let default_authority = if target.port == 443 {
            authority_host(&leaf_host)
        } else {
            format!("{}:{}", authority_host(&leaf_host), target.port)
        };
        let session = Session {
            access,
            logger: logger.clone(),
            tasks: tasks.clone(),
            shutdown: shutdown.clone(),
            client,
            default_authority,
            dialer,
            routing,
            error_page,
        };

        // 4. Serve decrypted HTTP.
        let fixed = match &session.routing {
            Routing::Fixed { decision, info } => Some((Arc::clone(decision), info.clone())),
            Routing::PerRequest(_) => None,
        };
        let result = match (upstream, fixed) {
            (Some(up), Some((decision, info))) if negotiated.as_deref() == Some(ALPN_H2) => {
                session.serve_h2(client_tls, up, decision, info).await
            }
            (up, _) => session.serve_h1(client_tls, up).await,
        };
        if let Err(e) = result {
            logger.debug(format!("MITM session {client} -> {target} ended: {e}"));
        }
        tasks.close();
        tasks.wait().await;
    }
}

fn authority_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn new_guard(
    access: &AccessLogger,
    tasks: &TaskTracker,
    client: SocketAddr,
    method: &str,
    target: String,
) -> AccessGuard {
    AccessGuard::new(
        access.clone(),
        tasks.clone(),
        HttpAccessRecord {
            ts: OffsetDateTime::now_utc(),
            direction: None,
            client,
            proto: "https".into(),
            mitm: true,
            method: method.to_string(),
            target,
            rule: String::new(),
            profile: String::new(),
            upstream: String::new(),
            status: 0,
            bytes_in: 0,
            bytes_out: 0,
            duration_ms: 0,
        },
    )
}

fn is_upgrade_request<B>(req: &Request<B>) -> bool {
    req.headers().contains_key(header::UPGRADE)
        && req
            .headers()
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
}

fn set_route(guard: &mut AccessGuard, info: &RouteInfo) {
    guard.set_route(&info.rule, info.profile.clone(), info.upstream.clone());
    if let Some(counters) = &info.counters {
        guard.track(counters, &info.rule, &info.counted_profile);
    }
}

impl Session {
    fn guard(&self, method: &str, target: String) -> AccessGuard {
        new_guard(&self.access, &self.tasks, self.client, method, target)
    }

    /// A proxy-generated error page for the request's `authority`.
    fn error(&self, status: StatusCode, msg: &str, authority: &str) -> Response<ProxyBody> {
        self.error_page
            .response(status, msg, page_target(authority))
    }

    /// `(authority, https:// URL)` for a decrypted request.
    fn request_target<B>(&self, req: &Request<B>) -> (String, String) {
        let authority = req
            .uri()
            .authority()
            .map(|a| a.as_str().to_string())
            .or_else(|| {
                req.headers()
                    .get(header::HOST)
                    .and_then(|h| h.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.default_authority.clone());
        let pq = req.uri().path_and_query().map_or("/", |pq| pq.as_str());
        let url = format!("https://{authority}{pq}");
        (authority, url)
    }

    fn spawn_driver<F>(&self, what: &'static str, conn: F)
    where
        F: std::future::Future<Output = Result<(), hyper::Error>> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        let logger = self.logger.clone();
        self.tasks.spawn(async move {
            tokio::select! {
                res = conn => {
                    if let Err(e) = res {
                        logger.debug(format!("MITM upstream {what} connection error: {e}"));
                    }
                }
                () = shutdown.cancelled() => {}
            }
        });
    }

    async fn h1_handshake(&self, tls: UpstreamTls) -> Result<H1Sender, hyper::Error> {
        let (sender, conn) = client_http1::handshake(TokioIo::new(tls)).await?;
        self.spawn_driver("HTTP/1.1", conn.with_upgrades());
        Ok(sender)
    }

    /// The decision for one decrypted request (`url` is its full `https://` URL) and the
    /// dialer bound to the snapshot it was made with.
    fn decide(&self, url: &str) -> Result<(Arc<RouteDecision>, Dialer), DialFailure> {
        match &self.routing {
            Routing::Fixed { decision, .. } => Ok((Arc::clone(decision), self.dialer.clone())),
            Routing::PerRequest(live) => {
                let route = live.pin();
                let dialer = Dialer {
                    state: route.state(),
                    route,
                    ..self.dialer.clone()
                };
                let decision = dialer.route(Some(url))?;
                Ok((Arc::new(decision), dialer))
            }
        }
    }

    /// The upstream HTTP/1.1 sender for `decision`: the attached one if it serves the same
    /// chain and is still open, otherwise a freshly dialed one.
    async fn h1_upstream(
        &self,
        slot: &Mutex<Option<H1Upstream>>,
        decision: &RouteDecision,
        dialer: &Dialer,
    ) -> Result<H1Upstream, DialFailure> {
        // Same chain under the same config generation: a reload may redefine profiles.
        let key = format!("{}@{}", chain_key(decision), dialer.state.config_generation);
        if let Some(mut up) = slot.lock().await.take() {
            if up.key == key && up.sender.ready().await.is_ok() {
                return Ok(up);
            }
        }
        let (tls, info) = dialer.connect(decision, vec![ALPN_HTTP11.to_vec()]).await?;
        match self.h1_handshake(tls).await {
            Ok(sender) => Ok(H1Upstream { sender, key, info }),
            Err(e) => {
                self.logger.error(format!(
                    "MITM upstream {} handshake failed: {e}",
                    self.dialer.target
                ));
                Err(DialFailure {
                    status: StatusCode::BAD_GATEWAY,
                    info,
                })
            }
        }
    }

    async fn serve_h1(
        &self,
        client_tls: ClientTls,
        upstream: Option<UpstreamTls>,
    ) -> Result<(), hyper::Error> {
        let first = match (upstream, &self.routing) {
            (Some(tls), Routing::Fixed { decision, info }) => Some(H1Upstream {
                sender: self.h1_handshake(tls).await?,
                key: format!(
                    "{}@{}",
                    chain_key(decision),
                    self.dialer.state.config_generation
                ),
                info: info.clone(),
            }),
            _ => None,
        };
        let slot = Arc::new(Mutex::new(first));
        let session = self.clone();
        let service = service_fn(move |req| {
            let session = session.clone();
            let slot = Arc::clone(&slot);
            async move { Ok::<_, Infallible>(session.forward_h1(req, &slot).await) }
        });
        let conn = server_http1::Builder::new()
            .serve_connection(TokioIo::new(client_tls), service)
            .with_upgrades();
        let mut conn = std::pin::pin!(conn);
        tokio::select! {
            res = conn.as_mut() => res,
            () = self.shutdown.cancelled() => {
                conn.as_mut().graceful_shutdown();
                conn.as_mut().await
            }
        }
    }

    async fn forward_h1(
        &self,
        mut req: Request<Incoming>,
        slot: &Mutex<Option<H1Upstream>>,
    ) -> Response<ProxyBody> {
        let (authority, url) = self.request_target(&req);
        let mut guard = self.guard(req.method().as_str(), url.clone());
        let (decision, dialer) = match self.decide(&url) {
            Ok(d) => d,
            Err(f) => {
                guard.set_status(f.status);
                return self.error(f.status, "destination not allowed", &authority);
            }
        };
        let upgrade = is_upgrade_request(&req).then(|| {
            let proto = req.headers().get(header::UPGRADE).cloned();
            (hyper::upgrade::on(&mut req), proto)
        });

        let (mut parts, body) = req.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        if let Some((_, Some(proto))) = &upgrade {
            parts.headers.insert(header::UPGRADE, proto.clone());
            parts
                .headers
                .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        }
        let origin = parts.uri.path_and_query().map_or("/", |pq| pq.as_str());
        parts.uri = origin
            .parse::<Uri>()
            .unwrap_or_else(|_| Uri::from_static("/"));
        parts.version = Version::HTTP_11;
        if !parts.headers.contains_key(header::HOST) {
            match HeaderValue::from_str(&authority) {
                Ok(v) => {
                    parts.headers.insert(header::HOST, v);
                }
                Err(_) => {
                    guard.set_status(StatusCode::BAD_REQUEST);
                    return self.error(StatusCode::BAD_REQUEST, "invalid host", &authority);
                }
            }
        }

        let mut up = match self.h1_upstream(slot, &decision, &dialer).await {
            Ok(up) => up,
            Err(f) => {
                set_route(&mut guard, &f.info);
                guard.set_status(f.status);
                let msg = if f.status == StatusCode::FORBIDDEN {
                    "destination blocked"
                } else {
                    "upstream connection failed"
                };
                return self.error(f.status, msg, &authority);
            }
        };
        set_route(&mut guard, &up.info);
        let body = CountingBody::new(body, guard.bytes_in(), None).boxed();
        let upstream_req = Request::from_parts(parts, body);
        let mut resp = match up.sender.send_request(upstream_req).await {
            Ok(r) => r,
            Err(e) => {
                self.logger.error(format!(
                    "MITM upstream request to {authority} failed: {}",
                    error_chain(&e)
                ));
                guard.set_status(StatusCode::BAD_GATEWAY);
                return self.error(
                    StatusCode::BAD_GATEWAY,
                    "upstream request failed",
                    &authority,
                );
            }
        };
        guard.set_status(resp.status());

        if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            if let Some((client_upgrade, _)) = upgrade {
                // The upstream connection now belongs to the upgraded stream.
                drop(up);
                let upstream_upgrade = hyper::upgrade::on(&mut resp);
                self.spawn_upgrade_relay(client_upgrade, upstream_upgrade, guard);
                let (parts, _) = resp.into_parts();
                return Response::from_parts(parts, empty_body());
            }
        }
        *slot.lock().await = Some(up);

        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        let bytes_out = guard.bytes_out();
        Response::from_parts(
            parts,
            CountingBody::new(body, bytes_out, Some(guard)).boxed(),
        )
    }

    fn spawn_upgrade_relay(
        &self,
        client: hyper::upgrade::OnUpgrade,
        upstream: hyper::upgrade::OnUpgrade,
        guard: AccessGuard,
    ) {
        let shutdown = self.shutdown.clone();
        let logger = self.logger.clone();
        let peer = self.client;
        self.tasks.spawn(async move {
            let (client, upstream) = match tokio::try_join!(client, upstream) {
                Ok(pair) => pair,
                Err(e) => {
                    logger.debug(format!("MITM upgrade for {peer} failed: {e}"));
                    return;
                }
            };
            let mut client =
                CountingStream::new(TokioIo::new(client), guard.bytes_in(), guard.bytes_out());
            let mut upstream = TokioIo::new(upstream);
            tokio::select! {
                res = relay_bidirectional(&mut client, &mut upstream) => {
                    if let Err(e) = res {
                        logger.debug(format!("MITM upgraded stream for {peer} ended: {e}"));
                    }
                }
                () = shutdown.cancelled() => {}
            }
            drop(guard);
        });
    }

    /// HTTP/2 to client and upstream (fixed routing only). The upstream connection is
    /// redialed when the origin closes it; see [`H2Tunnel`].
    async fn serve_h2(
        &self,
        client_tls: ClientTls,
        upstream: UpstreamTls,
        decision: Arc<RouteDecision>,
        info: RouteInfo,
    ) -> Result<(), hyper::Error> {
        let sender = self.h2_handshake(upstream).await?;
        let tunnel = Arc::new(H2Tunnel {
            decision,
            slot: Mutex::new(H2Slot {
                generation: 0,
                upstream: H2Upstream::Open { sender, info },
            }),
            goaway: tokio::sync::Notify::new(),
        });
        let session = self.clone();
        let shared = Arc::clone(&tunnel);
        let service = service_fn(move |req| {
            let session = session.clone();
            let tunnel = Arc::clone(&shared);
            async move { Ok::<_, Infallible>(session.forward_h2(req, &tunnel).await) }
        });
        let conn = server_http2::Builder::new(TokioExecutor::new())
            .max_header_list_size(H2_MAX_HEADER_LIST_SIZE)
            .serve_connection(TokioIo::new(client_tls), service);
        let mut conn = std::pin::pin!(conn);
        tokio::select! {
            res = conn.as_mut() => return res,
            () = self.shutdown.cancelled() => {}
            () = tunnel.goaway.notified() => {}
        }
        conn.as_mut().graceful_shutdown();
        conn.as_mut().await
    }

    async fn h2_handshake(&self, tls: UpstreamTls) -> Result<H2Sender, hyper::Error> {
        let (sender, conn) = client_http2::Builder::new(TokioExecutor::new())
            .max_header_list_size(H2_MAX_HEADER_LIST_SIZE)
            .handshake(TokioIo::new(tls))
            .await?;
        self.spawn_driver("HTTP/2", conn);
        Ok(sender)
    }

    /// The upstream for the next request on an HTTP/2 tunnel: the current connection if it
    /// is still open, otherwise a new one dialed through the tunnel's decision. A redial
    /// that no longer negotiates `h2` asks the client to reconnect (GOAWAY) and serves the
    /// request over HTTP/1.1.
    async fn h2_pick(&self, tunnel: &H2Tunnel) -> Result<H2Pick, DialFailure> {
        let mut slot = tunnel.slot.lock().await;
        match &slot.upstream {
            H2Upstream::Open { sender, info } if !sender.is_closed() => {
                return Ok(H2Pick::H2 {
                    sender: sender.clone(),
                    info: info.clone(),
                    generation: slot.generation,
                });
            }
            H2Upstream::Downgraded => {
                drop(slot);
                let (tls, info) = self
                    .dialer
                    .connect(&tunnel.decision, vec![ALPN_HTTP11.to_vec()])
                    .await?;
                return Ok(H2Pick::H1 {
                    tls: Box::new(tls),
                    info,
                });
            }
            H2Upstream::Open { .. } | H2Upstream::Closed => {}
        }
        // Dial while holding the slot, so concurrent requests share the new connection.
        let target = &self.dialer.target;
        self.logger.debug(format!(
            "MITM upstream HTTP/2 connection to {target} closed; redialing"
        ));
        let (tls, info) = self
            .dialer
            .connect(
                &tunnel.decision,
                vec![ALPN_H2.to_vec(), ALPN_HTTP11.to_vec()],
            )
            .await?;
        slot.generation += 1;
        if tls.get_ref().1.alpn_protocol() != Some(ALPN_H2) {
            self.logger.info(format!(
                "MITM upstream {target} no longer negotiates HTTP/2; asking client {} to \
                 reconnect",
                self.client
            ));
            slot.upstream = H2Upstream::Downgraded;
            tunnel.goaway.notify_one();
            return Ok(H2Pick::H1 {
                tls: Box::new(tls),
                info,
            });
        }
        match self.h2_handshake(tls).await {
            Ok(sender) => {
                slot.upstream = H2Upstream::Open {
                    sender: sender.clone(),
                    info: info.clone(),
                };
                Ok(H2Pick::H2 {
                    sender,
                    info,
                    generation: slot.generation,
                })
            }
            Err(e) => {
                self.logger.error(format!(
                    "MITM upstream {target} HTTP/2 handshake failed: {e}"
                ));
                slot.upstream = H2Upstream::Closed;
                Err(DialFailure {
                    status: StatusCode::BAD_GATEWAY,
                    info,
                })
            }
        }
    }

    /// Retire the connection of `generation`, unless a redial already replaced it.
    async fn h2_retire(tunnel: &H2Tunnel, generation: u64) {
        let mut slot = tunnel.slot.lock().await;
        if slot.generation == generation && matches!(slot.upstream, H2Upstream::Open { .. }) {
            slot.upstream = H2Upstream::Closed;
        }
    }

    async fn forward_h2(&self, req: Request<Incoming>, tunnel: &H2Tunnel) -> Response<ProxyBody> {
        let (authority, url) = self.request_target(&req);
        let mut guard = self.guard(req.method().as_str(), url.clone());
        let (mut parts, body) = req.into_parts();
        // `te: trailers` is the one TE value HTTP/2 allows, and gRPC needs it end to end.
        let te_trailers = accepts_trailers(&parts.headers);
        strip_hop_by_hop(&mut parts.headers);
        parts.headers.remove(header::HOST);
        parts.version = Version::HTTP_2;
        parts.uri = match url.parse::<Uri>() {
            Ok(u) => u,
            Err(_) => {
                guard.set_status(StatusCode::BAD_REQUEST);
                return self.error(
                    StatusCode::BAD_REQUEST,
                    "invalid request target",
                    &authority,
                );
            }
        };
        if te_trailers {
            parts
                .headers
                .insert(header::TE, HeaderValue::from_static("trailers"));
        }
        // A body-less idempotent request may be resent when the origin dropped its
        // connection before answering, as browsers do.
        let replay = (body.is_end_stream() && parts.method.is_idempotent()).then(|| parts.clone());
        let mut request = Request::from_parts(
            parts,
            CountingBody::new(body, guard.bytes_in(), None).boxed(),
        );
        let mut resent = false;
        loop {
            let (mut sender, info, generation) = match self.h2_pick(tunnel).await {
                Ok(H2Pick::H2 {
                    sender,
                    info,
                    generation,
                }) => (sender, info, generation),
                Ok(H2Pick::H1 { tls, info }) => {
                    set_route(&mut guard, &info);
                    return self
                        .forward_h2_over_h1(request, *tls, guard, &authority)
                        .await;
                }
                Err(f) => {
                    set_route(&mut guard, &f.info);
                    guard.set_status(f.status);
                    let msg = if f.status == StatusCode::FORBIDDEN {
                        "destination blocked"
                    } else {
                        "upstream connection failed"
                    };
                    return self.error(f.status, msg, &authority);
                }
            };
            let err = match sender.try_send_request(request).await {
                Ok(resp) => {
                    set_route(&mut guard, &info);
                    return relay_response(resp, guard);
                }
                Err(e) => e,
            };
            // Unsent: the connection closed before the request was written, so any request
            // may go out again. Dropped: written, but the connection went away unanswered.
            let mut err = err;
            let unsent = err.take_message();
            let dropped = upstream_dropped(err.error());
            if unsent.is_some() || dropped {
                Self::h2_retire(tunnel, generation).await;
            }
            let again = unsent.or_else(|| {
                replay
                    .clone()
                    .filter(|_| dropped)
                    .map(|p| Request::from_parts(p, empty_body()))
            });
            match again {
                Some(r) if !resent => {
                    self.logger.debug(format!(
                        "MITM upstream HTTP/2 connection to {authority} closed before \
                         answering; resending on a new connection"
                    ));
                    resent = true;
                    request = r;
                }
                _ => {
                    set_route(&mut guard, &info);
                    self.logger.error(format!(
                        "MITM upstream HTTP/2 request to {authority} failed: {}",
                        error_chain(err.error())
                    ));
                    guard.set_status(StatusCode::BAD_GATEWAY);
                    return self.error(
                        StatusCode::BAD_GATEWAY,
                        "upstream request failed",
                        &authority,
                    );
                }
            }
        }
    }

    /// Send one decrypted HTTP/2 request over a dedicated HTTP/1.1 upstream connection
    /// (the origin stopped negotiating `h2` mid-tunnel).
    async fn forward_h2_over_h1(
        &self,
        request: Request<ProxyBody>,
        tls: UpstreamTls,
        mut guard: AccessGuard,
        authority: &str,
    ) -> Response<ProxyBody> {
        let (mut parts, body) = request.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        let origin = parts.uri.path_and_query().map_or("/", |pq| pq.as_str());
        parts.uri = origin
            .parse::<Uri>()
            .unwrap_or_else(|_| Uri::from_static("/"));
        parts.version = Version::HTTP_11;
        match HeaderValue::from_str(authority) {
            Ok(v) => {
                parts.headers.insert(header::HOST, v);
            }
            Err(_) => {
                guard.set_status(StatusCode::BAD_REQUEST);
                return self.error(StatusCode::BAD_REQUEST, "invalid host", authority);
            }
        }
        let sent = match self.h1_handshake(tls).await {
            Ok(mut sender) => sender.send_request(Request::from_parts(parts, body)).await,
            Err(e) => Err(e),
        };
        match sent {
            Ok(resp) => {
                let mut resp = relay_response(resp, guard);
                *resp.version_mut() = Version::HTTP_2;
                resp
            }
            Err(e) => {
                self.logger.error(format!(
                    "MITM upstream HTTP/1.1 request to {authority} failed: {}",
                    error_chain(&e)
                ));
                guard.set_status(StatusCode::BAD_GATEWAY);
                self.error(
                    StatusCode::BAD_GATEWAY,
                    "upstream request failed",
                    authority,
                )
            }
        }
    }
}

/// Hand an upstream response to the client: hop-by-hop headers stripped, body counted, and
/// the access record completed when the body ends.
fn relay_response(resp: Response<Incoming>, mut guard: AccessGuard) -> Response<ProxyBody> {
    guard.set_status(resp.status());
    let (mut parts, body) = resp.into_parts();
    strip_hop_by_hop(&mut parts.headers);
    let bytes_out = guard.bytes_out();
    Response::from_parts(
        parts,
        CountingBody::new(body, bytes_out, Some(guard)).boxed(),
    )
}

/// `e` and each of its sources, joined with `: ` (hyper's own message is only the error
/// kind, such as `http2 error`; the cause, such as an HTTP/2 reset reason, is a source).
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(err) = source {
        let msg = err.to_string();
        if !out.ends_with(&msg) {
            out.push_str(": ");
            out.push_str(&msg);
        }
        source = err.source();
    }
    out
}

/// Whether the client's `TE` header lists `trailers`.
fn accepts_trailers(headers: &header::HeaderMap) -> bool {
    headers
        .get_all(header::TE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| t.split(';').next())
        .any(|t| t.trim().eq_ignore_ascii_case("trailers"))
}

/// Whether an upstream HTTP/2 request failed because the connection went away before an
/// answer: the origin refused the stream, sent GOAWAY, or the connection closed or reset.
/// The origin may still have started on such a request, so only body-less idempotent
/// requests are resent on this signal.
fn upstream_dropped(e: &hyper::Error) -> bool {
    if e.is_canceled() || e.is_closed() || e.is_incomplete_message() {
        return true;
    }
    let mut source = std::error::Error::source(e);
    while let Some(err) = source {
        if let Some(h2e) = err.downcast_ref::<h2::Error>() {
            if h2e.is_remote()
                && (h2e.is_go_away() || h2e.reason() == Some(h2::Reason::REFUSED_STREAM))
            {
                return true;
            }
            if h2e.is_io() {
                source = h2e
                    .get_io()
                    .map(|io| io as &(dyn std::error::Error + 'static));
                continue;
            }
            return false;
        }
        if let Some(io) = err.downcast_ref::<io::Error>() {
            return matches!(
                io.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::UnexpectedEof
            );
        }
        source = err.source();
    }
    false
}

fn empty_body() -> ProxyBody {
    Empty::new().map_err(|never| match never {}).boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::ResolvedTarget;

    fn target(profile: &str) -> ResolvedTarget {
        ResolvedTarget {
            name: profile.into(),
            profile: profile.into(),
            kind: crate::routing::ProfileType::Direct,
            bypassed: false,
        }
    }

    #[test]
    fn chain_key_identifies_failover_chains() {
        let decision = |targets: Vec<ResolvedTarget>| RouteDecision {
            table: crate::router::TableKind::Unified,
            rule: None,
            rule_index: None,
            targets,
            has_url_rules: true,
        };
        let a = decision(vec![target("corp"), target("direct")]);
        let b = decision(vec![target("corp"), target("direct")]);
        let c = decision(vec![target("direct")]);
        assert_eq!(chain_key(&a), "corp,direct");
        assert_eq!(chain_key(&a), chain_key(&b));
        assert_ne!(chain_key(&a), chain_key(&c));
    }

    #[test]
    fn authority_host_brackets_ipv6() {
        assert_eq!(authority_host("::1"), "[::1]");
        assert_eq!(authority_host("example.com"), "example.com");
    }

    /// Serve a fresh self-signed certificate (SAN `san`) over an in-memory pipe and run a
    /// client handshake with `verification` against `server_name`.
    async fn handshake(
        verification: &UpstreamTlsVerification,
        san: &str,
        server_name: &str,
    ) -> Result<(), io::Error> {
        let provider = crate::mitm::crypto_provider();
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(signing_key.serialize_der().into());
        let server_cfg = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], key)
            .unwrap();
        let client_cfg = upstream_client_config(provider, verification).unwrap();

        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_cfg));
        let server = tokio::spawn(async move { acceptor.accept(server_io).await.map(|_| ()) });
        let name = ServerName::try_from(server_name.to_string()).unwrap();
        let res = TlsConnector::from(Arc::new(client_cfg))
            .connect(name, client_io)
            .await
            .map(|_| ());
        let _ = server.await;
        res
    }

    fn rules(list: &[&str]) -> UpstreamTlsVerification {
        UpstreamTlsVerification::AllowInsecureHosts(
            list.iter()
                .map(|r| r.parse::<FilterRule>().unwrap())
                .collect(),
        )
    }

    #[tokio::test]
    async fn strict_rejects_self_signed_upstream() {
        let strict = UpstreamTlsVerification::Strict;
        assert!(handshake(&strict, "self-signed.test", "self-signed.test")
            .await
            .is_err());
        assert!(handshake(&strict, "10.1.2.3", "10.1.2.3").await.is_err());
        // An empty allowlist is strict.
        assert!(
            handshake(&rules(&[]), "self-signed.test", "self-signed.test")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn allowlist_accepts_only_matching_targets() {
        let allow = rules(&["exact.test", "*.wild.test", "10.1.2.3", "192.168.50.0/24"]);
        for host in ["exact.test", "EXACT.test", "a.wild.test", "a.b.wild.test"] {
            handshake(&allow, host, host)
                .await
                .unwrap_or_else(|e| panic!("{host}: {e}"));
        }
        for ip in ["10.1.2.3", "192.168.50.7"] {
            handshake(&allow, ip, ip)
                .await
                .unwrap_or_else(|e| panic!("{ip}: {e}"));
        }
        for host in [
            "other.test",
            "wild.test",
            "notexact.test",
            "10.1.2.4",
            "192.168.51.1",
        ] {
            assert!(
                handshake(&allow, host, host).await.is_err(),
                "{host} must be verified strictly"
            );
        }
    }

    #[tokio::test]
    async fn insecure_all_accepts_any_self_signed_upstream() {
        let all = UpstreamTlsVerification::InsecureAll;
        for host in ["self-signed.test", "anything.example", "10.9.8.7", "::1"] {
            handshake(&all, host, host)
                .await
                .unwrap_or_else(|e| panic!("{host}: {e}"));
        }
        // The certificate need not even name the target.
        handshake(&all, "unrelated.test", "target.test")
            .await
            .unwrap();
    }

    /// Server resolver presenting one certificate but signing with an unrelated key.
    #[derive(Debug)]
    struct MismatchedKey(Arc<CertifiedKey>);

    impl ResolvesServerCert for MismatchedKey {
        fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            Some(Arc::clone(&self.0))
        }
    }

    #[tokio::test]
    async fn insecure_modes_still_verify_handshake_signatures() {
        let provider = crate::mitm::crypto_provider();
        let presented = rcgen::generate_simple_self_signed(vec!["sig.test".to_string()]).unwrap();
        let other = rcgen::KeyPair::generate().unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(other.serialize_der().into());
        let signing_key = provider.key_provider.load_private_key(key).unwrap();
        let certified = CertifiedKey::new(vec![presented.cert.der().clone()], signing_key);
        let server_cfg = Arc::new(
            ServerConfig::builder_with_provider(Arc::clone(&provider))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(MismatchedKey(Arc::new(certified)))),
        );
        for verification in [UpstreamTlsVerification::InsecureAll, rules(&["sig.test"])] {
            let client_cfg = upstream_client_config(Arc::clone(&provider), &verification).unwrap();
            let (client_io, server_io) = tokio::io::duplex(64 * 1024);
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::clone(&server_cfg));
            let server = tokio::spawn(async move { acceptor.accept(server_io).await.map(|_| ()) });
            let res = TlsConnector::from(Arc::new(client_cfg))
                .connect(ServerName::try_from("sig.test").unwrap(), client_io)
                .await;
            let _ = server.await;
            assert!(
                res.is_err(),
                "{verification:?} accepted a bad handshake signature"
            );
        }
    }
}
