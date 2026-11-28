//! Upstream dialer and proxy chaining layer.
//!
//! Every outbound connection goes through an [`UpstreamDialer`], which returns an
//! [`UpstreamStream`]: a plain TCP socket, a TLS session (when talking to an `https` proxy),
//! a [`PrefixedStream`] that first replays bytes read past an HTTP `CONNECT` response, or a
//! [`UpstreamStream::Boxed`] stream supplied by an external dialer (SSH channel, in-memory
//! pipe, ...).
//!
//! Built-in dialers:
//! - [`DirectDialer`] (`direct`): OS/Tokio DNS, `TCP_NODELAY`, TCP keepalive.
//! - [`BlockDialer`] (`block`): typed [`RouteBlocked`] error without DNS or network I/O.
//! - [`Socks5Dialer`] (`socks5`): RFC 1928 + optional RFC 1929 auth, remote DNS (`ATYP=0x03`).
//! - [`Socks4aDialer`] (`socks4a`): SOCKS4a remote DNS (`0.0.0.1` + hostname).
//! - [`HttpConnectDialer`] (`http` and `https`): `CONNECT host:port` + optional Basic auth.
//!
//! Hostnames routed to a proxy are always sent **unresolved**. [`FailoverDialer`] walks an
//! ordered chain of dialers, advancing only on TCP connect / proxy handshake failure (never
//! on [`RouteBlocked`]). Credentials never appear in `Debug` output, logs, or error strings.

pub mod block;
pub mod direct;
pub mod http;
pub mod socks4a;
pub mod socks5;

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::filter::HostFilter;
use crate::logging::{log_debug, Logger};
use crate::net::{KeepaliveSettings, PrefixedStream};
use crate::router::ResolvedTarget;
use crate::routing::{ProfileConfig, ProfileType, RoutingError, RoutingInput};

pub use block::BlockDialer;
pub use direct::DirectDialer;
pub use http::{HttpConnectDialer, TlsOptions};
pub use socks4a::Socks4aDialer;
pub use socks5::Socks5Dialer;

/// A connected upstream byte stream.
pub enum UpstreamStream {
    /// Plain TCP (direct dials, SOCKS proxies, `http` proxies).
    Tcp(TcpStream),
    /// TLS to an `https` proxy (boxed: the session state is large).
    Tls(Box<TlsStream<TcpStream>>),
    /// A stream with bytes read past a framing header (HTTP `CONNECT` response) to replay
    /// before the inner stream.
    Prefixed(Box<PrefixedStream<UpstreamStream>>),
    /// Any other byte stream, typically returned by an external [`UpstreamDialer`].
    Boxed(Box<dyn BoxedIo>),
}

/// Object-safe bound for [`UpstreamStream::Boxed`]: any `AsyncRead + AsyncWrite + Send +
/// Unpin` stream (blanket-implemented).
pub trait BoxedIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin + ?Sized> BoxedIo for T {}

impl UpstreamStream {
    /// Wrap an arbitrary byte stream as [`UpstreamStream::Boxed`].
    pub fn boxed<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        Self::Boxed(Box::new(stream))
    }

    /// Wrap `self` so `buffered` bytes are read first; returns `self` unchanged when empty.
    pub fn with_prefix(self, buffered: Vec<u8>) -> Self {
        if buffered.is_empty() {
            self
        } else {
            Self::Prefixed(Box::new(PrefixedStream::new(self, buffered)))
        }
    }
}

impl fmt::Debug for UpstreamStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(s) => f.debug_tuple("Tcp").field(s).finish(),
            Self::Tls(s) => f.debug_tuple("Tls").field(s.get_ref().0).finish(),
            Self::Prefixed(p) => f
                .debug_struct("Prefixed")
                .field("buffered", &p.buffered())
                .finish_non_exhaustive(),
            Self::Boxed(_) => f.debug_tuple("Boxed").finish_non_exhaustive(),
        }
    }
}

impl AsyncRead for UpstreamStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            Self::Prefixed(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            Self::Boxed(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for UpstreamStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            Self::Prefixed(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            Self::Boxed(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
            Self::Prefixed(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
            Self::Boxed(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(s) => s.is_write_vectored(),
            Self::Tls(s) => s.is_write_vectored(),
            Self::Prefixed(s) => s.is_write_vectored(),
            Self::Boxed(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            Self::Prefixed(s) => Pin::new(s.as_mut()).poll_flush(cx),
            Self::Boxed(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            Self::Prefixed(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            Self::Boxed(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Boxed future returned by [`UpstreamDialer::connect`].
pub type DialFuture<'a> = Pin<Box<dyn Future<Output = io::Result<UpstreamStream>> + Send + 'a>>;

/// Opens upstream connections to `host:port`.
pub trait UpstreamDialer: Send + Sync {
    /// Connect to `host:port` (a DNS name or IP literal; IPv6 with or without brackets),
    /// bounding the whole operation — TCP connect plus any proxy handshake — by `timeout`.
    fn connect(&self, host: &str, port: u16, timeout: Duration) -> DialFuture<'_>;

    /// Redacted description of the upstream (`socks5://127.0.0.1:1081`, `direct`, `block`)
    /// for logs; never carries credentials.
    fn redacted_upstream(&self) -> String;
}

/// Typed error returned by [`BlockDialer`]: the route resolved to a `block` profile.
///
/// Carried inside an [`io::Error`] of kind [`io::ErrorKind::PermissionDenied`]; detect it
/// with [`is_route_blocked`] / [`RouteBlocked::from_io`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("route blocked by profile `{profile}`")]
pub struct RouteBlocked {
    pub profile: String,
}

impl RouteBlocked {
    /// Wrap as an [`io::Error`] of kind `PermissionDenied`.
    pub fn into_io(self) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, self)
    }

    /// The [`RouteBlocked`] carried by `err`, if any.
    pub fn from_io(err: &io::Error) -> Option<&RouteBlocked> {
        err.get_ref().and_then(|e| e.downcast_ref::<RouteBlocked>())
    }
}

/// Whether `err` is a [`RouteBlocked`] error.
pub fn is_route_blocked(err: &io::Error) -> bool {
    RouteBlocked::from_io(err).is_some()
}

/// Typed classification of an upstream dial failure.
///
/// Dialers may carry a `DialError` inside an [`io::Error`] (see [`DialError::into_io`]);
/// [`dial_error`] classifies any dial `io::Error` — carried `DialError`, legacy
/// [`RouteBlocked`], filter [`AccessDenied`](crate::filter::AccessDenied), or a bare
/// [`io::ErrorKind`] — so the HTTP (`403`/`504`/`502`) and SOCKS reply-code mappings agree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DialError {
    /// The route resolved to a `block` profile.
    #[error("route blocked by profile `{profile}`")]
    Blocked { profile: String },
    /// The destination was denied by the global destination filter.
    #[error("{reason}")]
    Filtered { reason: String },
    /// The upstream (or the destination) could not be reached; `kind` keeps the precise
    /// cause (`ConnectionRefused`, `HostUnreachable`, `NetworkUnreachable`, ...).
    #[error("{message}")]
    NotConnected {
        kind: io::ErrorKind,
        message: String,
    },
    /// An upstream proxy rejected our credentials or offered no acceptable auth method.
    #[error("{message}")]
    AuthRejected { message: String },
    /// The dial (TCP connect plus proxy handshake) timed out.
    #[error("{message}")]
    Timeout { message: String },
    /// Any other failure.
    #[error("{message}")]
    Other {
        kind: io::ErrorKind,
        message: String,
    },
}

impl DialError {
    /// The [`io::ErrorKind`] this error is carried with.
    pub fn io_kind(&self) -> io::ErrorKind {
        match self {
            Self::Blocked { .. } | Self::Filtered { .. } | Self::AuthRejected { .. } => {
                io::ErrorKind::PermissionDenied
            }
            Self::NotConnected { kind, .. } | Self::Other { kind, .. } => *kind,
            Self::Timeout { .. } => io::ErrorKind::TimedOut,
        }
    }

    /// Wrap as an [`io::Error`] (kind from [`Self::io_kind`]). `Blocked` is carried as a
    /// [`RouteBlocked`] and `Filtered` as an [`AccessDenied`](crate::filter::AccessDenied)
    /// so [`is_route_blocked`] / [`crate::filter::is_access_denied`] keep working.
    pub fn into_io(self) -> io::Error {
        match self {
            Self::Blocked { profile } => RouteBlocked { profile }.into_io(),
            Self::Filtered { reason } => {
                crate::filter::access_denied_error(crate::filter::AccessDenied(reason))
            }
            other => io::Error::new(other.io_kind(), other),
        }
    }

    /// Whether this failure is a policy refusal (`Blocked` or `Filtered`).
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Blocked { .. } | Self::Filtered { .. })
    }
}

/// Classify a dial [`io::Error`] as a [`DialError`].
pub fn dial_error(err: &io::Error) -> DialError {
    if let Some(inner) = err.get_ref() {
        if let Some(d) = inner.downcast_ref::<DialError>() {
            return d.clone();
        }
        if let Some(b) = inner.downcast_ref::<RouteBlocked>() {
            return DialError::Blocked {
                profile: b.profile.clone(),
            };
        }
        if let Some(a) = inner.downcast_ref::<crate::filter::AccessDenied>() {
            return DialError::Filtered {
                reason: a.to_string(),
            };
        }
    }
    let message = err.to_string();
    match err.kind() {
        io::ErrorKind::TimedOut => DialError::Timeout { message },
        kind @ (io::ErrorKind::ConnectionRefused
        | io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable) => DialError::NotConnected { kind, message },
        kind => DialError::Other { kind, message },
    }
}

/// An [`io::Error`] carrying [`DialError::AuthRejected`] (kind `PermissionDenied`).
pub(crate) fn auth_rejected(message: impl Into<String>) -> io::Error {
    DialError::AuthRejected {
        message: message.into(),
    }
    .into_io()
}

/// Bounds a dial future by `timeout`, mapping expiry to [`io::ErrorKind::TimedOut`].
///
/// `what` describes the operation for the timeout error; it is evaluated only when the
/// timeout expires, so successful dials allocate no error context.
pub(crate) async fn with_timeout<W, F, T>(what: W, timeout: Duration, fut: F) -> io::Result<T>
where
    W: FnOnce() -> String,
    F: Future<Output = io::Result<T>>,
{
    tokio::time::timeout(timeout, fut).await.map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "{} timed out after {}",
                what(),
                humantime::format_duration(timeout)
            ),
        )
    })?
}

/// Strip `[...]` from a bracketed IPv6 literal.
pub(crate) fn strip_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

/// `host:port`, bracketing IPv6 literals.
pub(crate) fn host_port(host: &str, port: u16) -> String {
    let bare = strip_brackets(host);
    if bare.contains(':') {
        format!("[{bare}]:{port}")
    } else {
        format!("{bare}:{port}")
    }
}

/// A successful dial: the stream plus metadata for access logging.
#[derive(Debug)]
pub struct DialOutcome {
    pub stream: UpstreamStream,
    /// Name of the profile that connected.
    pub profile: String,
    /// Redacted upstream URI of that profile (no credentials).
    pub upstream: String,
    /// 0-based position of the successful attempt within the chain.
    pub attempt: usize,
}

/// Error building a dialer from a profile.
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error(transparent)]
    Routing(#[from] RoutingError),
    #[error("profile `{profile}`: {message}")]
    Profile { profile: String, message: String },
    #[error("unknown profile `{0}`")]
    UnknownProfile(String),
}

/// Settings shared by every dialer built from configuration.
#[derive(Debug, Clone, Default)]
pub struct DialerOptions {
    /// TCP keepalive on upstream sockets (`None` disables).
    pub keepalive: Option<KeepaliveSettings>,
    /// Destination filter enforced by `direct` dials (domain and resolved IP).
    pub filter: Option<Arc<HostFilter>>,
    /// Extra trust anchors for `https` proxies (when non-empty they replace the OS store).
    pub tls_roots: Vec<CertificateDer<'static>>,
    /// Caller-supplied dialers for `external` profiles, keyed by profile name (API-only;
    /// see [`ProfileType::External`]).
    pub external: ProfileRegistry,
}

impl DialerOptions {
    /// Supply the dialer for the `external` profile `name`.
    pub fn with_external(
        mut self,
        name: impl Into<String>,
        dialer: Arc<dyn UpstreamDialer>,
    ) -> Self {
        self.external.insert(name, dialer);
        self
    }
}

/// Build the dialer for one profile. `composite` profiles have no dialer of their own (the
/// router resolves them to a concrete profile first).
pub fn dialer_for_profile(
    name: &str,
    profile: &ProfileConfig,
    opts: &DialerOptions,
) -> Result<Option<Arc<dyn UpstreamDialer>>, UpstreamError> {
    let endpoint = || -> Result<(String, u16), UpstreamError> {
        match (&profile.host, profile.port) {
            (Some(h), Some(p)) => Ok((h.clone(), p)),
            _ => Err(UpstreamError::Profile {
                profile: name.to_string(),
                message: format!("type `{}` requires host and port", profile.kind),
            }),
        }
    };
    let credentials = || -> Result<Option<(String, String)>, UpstreamError> {
        let password = profile.resolve_password(name)?;
        Ok(match (&profile.username, password) {
            (Some(u), p) => Some((u.clone(), p.unwrap_or_default())),
            (None, Some(p)) => Some((String::new(), p)),
            (None, None) => None,
        })
    };
    let dialer: Arc<dyn UpstreamDialer> = match profile.kind {
        ProfileType::Direct => {
            let mut d = DirectDialer::new().with_tcp_keepalive(opts.keepalive);
            if let Some(f) = &opts.filter {
                d = d.with_filter(f.clone());
            }
            Arc::new(d)
        }
        ProfileType::Block => Arc::new(BlockDialer::new(name)),
        ProfileType::Socks5 => {
            let (h, p) = endpoint()?;
            Arc::new(
                Socks5Dialer::new(h, p)
                    .with_credentials(credentials()?)
                    .with_tcp_keepalive(opts.keepalive),
            )
        }
        ProfileType::Socks4a => {
            let (h, p) = endpoint()?;
            Arc::new(
                Socks4aDialer::new(h, p)
                    .with_user_id(profile.username.clone().unwrap_or_default())
                    .with_tcp_keepalive(opts.keepalive),
            )
        }
        ProfileType::Http => {
            let (h, p) = endpoint()?;
            Arc::new(
                HttpConnectDialer::http(h, p)
                    .with_credentials(credentials()?)
                    .with_tcp_keepalive(opts.keepalive),
            )
        }
        ProfileType::Https => {
            let (h, p) = endpoint()?;
            let tls = TlsOptions {
                insecure_skip_verify: profile.tls_insecure_skip_verify,
                roots: opts.tls_roots.clone(),
            };
            let d = HttpConnectDialer::https(h, p, &tls).map_err(|e| UpstreamError::Profile {
                profile: name.to_string(),
                message: format!("TLS setup failed: {e}"),
            })?;
            Arc::new(
                d.with_credentials(credentials()?)
                    .with_tcp_keepalive(opts.keepalive),
            )
        }
        ProfileType::External => match opts.external.get(name) {
            Some(d) => d.clone(),
            None => {
                return Err(UpstreamError::Profile {
                    profile: name.to_string(),
                    message: "type `external` requires a dialer supplied through \
                              `DialerOptions::with_external` (none was registered)"
                        .to_string(),
                })
            }
        },
        ProfileType::Composite => return Ok(None),
    };
    Ok(Some(dialer))
}

/// Default passive failover cooldown: how long a profile that failed to connect is skipped
/// by multi-target chains before it is probed again.
pub const DEFAULT_FAILOVER_COOLDOWN: Duration = Duration::from_secs(3);

/// Shared passive-health state: one failure timestamp per profile, in milliseconds since
/// `epoch` (`0` = healthy). Holds no sockets, DNS answers, or other connection data.
#[derive(Debug)]
struct CooldownState {
    epoch: Instant,
    cooldown: Duration,
}

impl CooldownState {
    fn new(cooldown: Duration) -> Self {
        Self {
            epoch: Instant::now(),
            cooldown,
        }
    }

    /// Milliseconds since `epoch`, never `0` (reserved for "healthy").
    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .max(1)
    }

    fn cooldown_ms(&self) -> u64 {
        u64::try_from(self.cooldown.as_millis()).unwrap_or(u64::MAX)
    }

    /// Whether a profile whose failure mark is `mark` is still cooling down at `now`.
    fn cooling(&self, mark: u64, now: u64) -> bool {
        mark != 0 && now.saturating_sub(mark) < self.cooldown_ms()
    }
}

/// Dialers for every concrete profile, keyed by profile name, plus the per-profile passive
/// failover cooldown shared by every [`FailoverDialer`] built from this registry (clones
/// share it too).
#[derive(Clone)]
pub struct ProfileRegistry {
    dialers: BTreeMap<String, Arc<dyn UpstreamDialer>>,
    health: BTreeMap<String, Arc<AtomicU64>>,
    cooldown: Arc<CooldownState>,
}

impl Default for ProfileRegistry {
    fn default() -> Self {
        Self {
            dialers: BTreeMap::new(),
            health: BTreeMap::new(),
            cooldown: Arc::new(CooldownState::new(DEFAULT_FAILOVER_COOLDOWN)),
        }
    }
}

impl fmt::Debug for ProfileRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.dialers.iter().map(|(k, d)| (k, d.redacted_upstream())))
            .finish()
    }
}

impl ProfileRegistry {
    /// Build dialers for `profiles` (resolving password sources once, up front).
    pub fn from_profiles(
        profiles: &BTreeMap<String, ProfileConfig>,
        opts: &DialerOptions,
    ) -> Result<Self, UpstreamError> {
        let mut reg = Self::default();
        for (name, profile) in profiles {
            if let Some(d) = dialer_for_profile(name, profile, opts)? {
                reg.insert(name.clone(), d);
            }
        }
        Ok(reg)
    }

    /// Set the passive failover cooldown (default [`DEFAULT_FAILOVER_COOLDOWN`]): after a
    /// profile fails to connect, multi-target chains skip it for this long, then let one
    /// connection probe it again. [`Duration::ZERO`] disables skipping. Resets all
    /// recorded failures.
    pub fn with_failover_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = Arc::new(CooldownState::new(cooldown));
        for mark in self.health.values_mut() {
            *mark = Arc::new(AtomicU64::new(0));
        }
        self
    }

    /// The passive failover cooldown applied by chains built from this registry.
    pub fn failover_cooldown(&self) -> Duration {
        self.cooldown.cooldown
    }

    /// Build from a validated [`RoutingInput`] (declared plus built-in profiles), using its
    /// TCP keepalive settings.
    pub fn from_input(input: &RoutingInput, opts: DialerOptions) -> Result<Self, UpstreamError> {
        let opts = DialerOptions {
            keepalive: input.keepalive,
            ..opts
        };
        Self::from_profiles(&input.profiles_with_builtins(), &opts)
    }

    /// Builder form of [`Self::insert`] for caller-supplied (`external`) dialers.
    pub fn with_external(
        mut self,
        name: impl Into<String>,
        dialer: Arc<dyn UpstreamDialer>,
    ) -> Self {
        self.insert(name, dialer);
        self
    }

    /// Register (or replace) a dialer under `name`.
    pub fn insert(&mut self, name: impl Into<String>, dialer: Arc<dyn UpstreamDialer>) {
        let name = name.into();
        self.health
            .insert(name.clone(), Arc::new(AtomicU64::new(0)));
        self.dialers.insert(name, dialer);
    }

    /// The dialer for `name`.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn UpstreamDialer>> {
        self.dialers.get(name)
    }

    /// Number of registered dialers.
    pub fn len(&self) -> usize {
        self.dialers.len()
    }

    /// Whether no dialer is registered.
    pub fn is_empty(&self) -> bool {
        self.dialers.is_empty()
    }

    /// A [`FailoverDialer`] over profile `names`, in order.
    pub fn chain<S: AsRef<str>>(&self, names: &[S]) -> Result<FailoverDialer, UpstreamError> {
        let mut entries = Vec::with_capacity(names.len());
        let mut health = Vec::with_capacity(names.len());
        for name in names {
            let name = name.as_ref();
            let dialer = self
                .get(name)
                .ok_or_else(|| UpstreamError::UnknownProfile(name.to_string()))?;
            entries.push((name.to_string(), dialer.clone()));
            health.push(self.health.get(name).cloned());
        }
        let mut chain = FailoverDialer::new(entries);
        if !self.cooldown.cooldown.is_zero() {
            chain.health = health;
            chain.cooldown = Some(self.cooldown.clone());
        }
        Ok(chain)
    }

    /// A [`FailoverDialer`] over a router decision's resolved targets (`profile` names).
    pub fn chain_for(&self, targets: &[ResolvedTarget]) -> Result<FailoverDialer, UpstreamError> {
        let names: Vec<&str> = targets.iter().map(|t| t.profile.as_str()).collect();
        self.chain(&names)
    }
}

/// Tries an ordered chain of dialers until one connects.
///
/// Each attempt is bounded by the dial timeout. A failure advances to the next entry
/// (logged at `DEBUG`), except [`RouteBlocked`], which stops the chain immediately. Failover
/// happens only while connecting: once a stream is returned the chain is done.
///
/// Chains built by [`ProfileRegistry::chain`] also apply the registry's passive failover
/// cooldown: a profile whose dial failed (connect error or timeout, never [`RouteBlocked`]
/// or access denial) is skipped by multi-target chains until the cooldown elapses; then a
/// single connection claims the probe and a success clears the mark. Single-target chains
/// never skip, and when every entry is cooling down all are still tried in order.
#[derive(Clone)]
pub struct FailoverDialer {
    entries: Vec<(String, Arc<dyn UpstreamDialer>)>,
    logger: Option<Logger>,
    /// Per-entry failure marks (parallel to `entries`; empty when cooldown is off).
    health: Vec<Option<Arc<AtomicU64>>>,
    cooldown: Option<Arc<CooldownState>>,
}

impl fmt::Debug for FailoverDialer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(
                self.entries
                    .iter()
                    .map(|(n, d)| format!("{n}={}", d.redacted_upstream())),
            )
            .finish()
    }
}

impl FailoverDialer {
    /// A chain of `(profile name, dialer)` entries, tried in order.
    pub fn new(entries: Vec<(String, Arc<dyn UpstreamDialer>)>) -> Self {
        Self {
            entries,
            logger: None,
            health: Vec::new(),
            cooldown: None,
        }
    }

    /// Failure mark and cooldown state for entry `i`, when passive cooldown applies.
    fn mark(&self, i: usize) -> Option<(&AtomicU64, &CooldownState)> {
        let state = self.cooldown.as_deref()?;
        let mark = self.health.get(i)?.as_deref()?;
        Some((mark, state))
    }

    /// Whether entry `i` should be skipped right now. Claims the probe slot (via
    /// compare-exchange) when its cooldown has elapsed, so only one connection probes.
    fn should_skip(&self, i: usize) -> bool {
        if self.entries.len() <= 1 {
            return false;
        }
        let Some((mark, state)) = self.mark(i) else {
            return false;
        };
        let now = state.now_ms();
        let seen = mark.load(Ordering::Acquire);
        if seen == 0 {
            return false;
        }
        if state.cooling(seen, now) {
            // Skip only while some other entry is still available.
            return (0..self.entries.len())
                .filter(|&j| j != i)
                .any(|j| match self.mark(j) {
                    Some((m, _)) => !state.cooling(m.load(Ordering::Acquire), now),
                    None => true,
                });
        }
        // Cooldown elapsed: claim the probe; losers keep failing over.
        mark.compare_exchange(seen, now, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    }

    fn record(&self, i: usize, ok: bool) {
        if let Some((mark, state)) = self.mark(i) {
            mark.store(if ok { 0 } else { state.now_ms() }, Ordering::Release);
        }
    }

    /// Log failed attempts at `DEBUG` through `logger`.
    pub fn with_logger(mut self, logger: Logger) -> Self {
        self.logger = Some(logger);
        self
    }

    /// Number of entries in the chain.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Dial `host:port` through the chain, returning the first successful stream plus which
    /// profile connected. When every entry fails, returns the last error with a summary.
    pub async fn dial(&self, host: &str, port: u16, timeout: Duration) -> io::Result<DialOutcome> {
        let mut failures = Vec::new();
        let mut last_err: Option<io::Error> = None;
        let mut skipped = Vec::new();
        let mut order: Vec<usize> = Vec::with_capacity(self.entries.len());
        for i in 0..self.entries.len() {
            if self.should_skip(i) {
                if let Some(logger) = &self.logger {
                    let (name, dialer) = &self.entries[i];
                    log_debug!(
                        logger,
                        "failover: skipping profile={name} upstream={} target={} \
                         (failed within cooldown)",
                        dialer.redacted_upstream(),
                        host_port(host, port)
                    );
                }
                skipped.push(i);
            } else {
                order.push(i);
            }
        }
        // Skipped entries stay a last resort, tried in order after every live one.
        order.extend(skipped);
        for attempt in order {
            let (name, dialer) = &self.entries[attempt];
            let upstream = dialer.redacted_upstream();
            let result = with_timeout(
                || format!("dial {} via {upstream}", host_port(host, port)),
                timeout,
                dialer.connect(host, port, timeout),
            )
            .await;
            match result {
                Ok(stream) => {
                    self.record(attempt, true);
                    return Ok(DialOutcome {
                        stream,
                        profile: name.clone(),
                        upstream,
                        attempt,
                    });
                }
                Err(e) if is_route_blocked(&e) => return Err(e),
                Err(e) => {
                    if !crate::filter::is_access_denied(&e) {
                        self.record(attempt, false);
                    }
                    if let Some(logger) = &self.logger {
                        log_debug!(
                            logger,
                            "failover: profile={name} upstream={upstream} target={} \
                             attempt={attempt} error={e}",
                            host_port(host, port)
                        );
                    }
                    failures.push(format!("{name}: {e}"));
                    last_err = Some(e);
                }
            }
        }
        Err(match last_err {
            None => io::Error::new(io::ErrorKind::InvalidInput, "failover chain has no targets"),
            Some(e) if failures.len() == 1 => e,
            Some(e) => io::Error::new(
                e.kind(),
                format!(
                    "all {} upstream targets failed for {}: {}",
                    failures.len(),
                    host_port(host, port),
                    failures.join("; ")
                ),
            ),
        })
    }
}

impl UpstreamDialer for FailoverDialer {
    fn connect(&self, host: &str, port: u16, timeout: Duration) -> DialFuture<'_> {
        let host = host.to_string();
        Box::pin(async move { self.dial(&host, port, timeout).await.map(|o| o.stream) })
    }

    fn redacted_upstream(&self) -> String {
        self.entries
            .iter()
            .map(|(_, d)| d.redacted_upstream())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_blocked_round_trips_through_io_error() {
        let err = RouteBlocked {
            profile: "block".into(),
        }
        .into_io();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(is_route_blocked(&err));
        assert_eq!(RouteBlocked::from_io(&err).unwrap().profile, "block");
        assert!(!is_route_blocked(&io::Error::other("x")));
    }

    #[test]
    fn dial_error_classifies_and_round_trips() {
        let blocked = DialError::Blocked {
            profile: "block".into(),
        }
        .into_io();
        assert!(is_route_blocked(&blocked));
        assert_eq!(
            dial_error(&blocked),
            DialError::Blocked {
                profile: "block".into()
            }
        );
        let denied = DialError::Filtered {
            reason: "nope".into(),
        }
        .into_io();
        assert!(crate::filter::is_access_denied(&denied));
        assert!(matches!(dial_error(&denied), DialError::Filtered { .. }));
        assert!(dial_error(&denied).is_denied());
        let auth = auth_rejected("bad creds");
        assert_eq!(auth.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(auth.to_string(), "bad creds");
        assert!(!is_route_blocked(&auth));
        assert!(matches!(dial_error(&auth), DialError::AuthRejected { .. }));
        let t = io::Error::new(io::ErrorKind::TimedOut, "slow");
        assert!(matches!(dial_error(&t), DialError::Timeout { .. }));
        let r = io::Error::new(io::ErrorKind::ConnectionRefused, "no");
        assert_eq!(
            dial_error(&r),
            DialError::NotConnected {
                kind: io::ErrorKind::ConnectionRefused,
                message: "no".into()
            }
        );
        let o = io::Error::other("x");
        assert!(matches!(dial_error(&o), DialError::Other { .. }));
        // Typed errors survive the io::Error round trip with their kind.
        let carried = DialError::NotConnected {
            kind: io::ErrorKind::HostUnreachable,
            message: "h".into(),
        }
        .into_io();
        assert_eq!(carried.kind(), io::ErrorKind::HostUnreachable);
        assert!(matches!(
            dial_error(&carried),
            DialError::NotConnected { .. }
        ));
    }

    #[test]
    fn host_port_brackets_ipv6() {
        assert_eq!(host_port("::1", 80), "[::1]:80");
        assert_eq!(host_port("[::1]", 80), "[::1]:80");
        assert_eq!(host_port("a.example", 443), "a.example:443");
    }

    #[tokio::test]
    async fn boxed_stream_relays_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (near, mut far) = tokio::io::duplex(64);
        let mut s = UpstreamStream::boxed(near);
        assert!(format!("{s:?}").starts_with("Boxed"));
        s.write_all(b"ping").await.unwrap();
        s.flush().await.unwrap();
        let mut b = [0u8; 4];
        far.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"ping");
        far.write_all(b"pong").await.unwrap();
        s.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");
        // Boxed streams compose with prefix replay like any other upstream stream.
        let mut p = s.with_prefix(b"pre".to_vec());
        far.write_all(b"!").await.unwrap();
        let mut b = [0u8; 4];
        p.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pre!");
        p.shutdown().await.unwrap();
        let mut rest = Vec::new();
        far.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }

    /// External test dialer: hands out one end of an in-memory pipe, or always fails.
    struct PipeDialer {
        fail: bool,
    }

    impl UpstreamDialer for PipeDialer {
        fn connect(&self, _host: &str, _port: u16, _timeout: Duration) -> DialFuture<'_> {
            let fail = self.fail;
            Box::pin(async move {
                if fail {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "pipe down",
                    ));
                }
                let (near, far) = tokio::io::duplex(64);
                tokio::spawn(async move {
                    let (mut r, mut w) = tokio::io::split(far);
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
                Ok(UpstreamStream::boxed(near))
            })
        }

        fn redacted_upstream(&self) -> String {
            "external:pipe".to_string()
        }
    }

    fn external_profiles() -> BTreeMap<String, ProfileConfig> {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "ssh".to_string(),
            ProfileConfig::of_kind(ProfileType::External),
        );
        profiles.insert(
            "direct".to_string(),
            ProfileConfig::of_kind(ProfileType::Direct),
        );
        profiles
    }

    #[test]
    fn external_profile_without_dialer_is_a_clear_error() {
        let err = ProfileRegistry::from_profiles(&external_profiles(), &DialerOptions::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("`ssh`") && err.contains("external"), "{err}");
    }

    #[tokio::test]
    async fn external_profile_with_supplied_dialer_connects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let opts =
            DialerOptions::default().with_external("ssh", Arc::new(PipeDialer { fail: false }));
        let reg = ProfileRegistry::from_profiles(&external_profiles(), &opts).unwrap();
        assert_eq!(reg.get("ssh").unwrap().redacted_upstream(), "external:pipe");
        let mut out = reg
            .chain(&["ssh"])
            .unwrap()
            .dial("db.internal", 5432, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(out.profile, "ssh");
        assert!(matches!(out.stream, UpstreamStream::Boxed(_)));
        out.stream.write_all(b"hi").await.unwrap();
        let mut b = [0u8; 2];
        out.stream.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"hi");
        // The registry builder registers external dialers directly as well.
        let reg =
            ProfileRegistry::default().with_external("x", Arc::new(PipeDialer { fail: false }));
        assert!(reg.get("x").is_some());
    }

    #[tokio::test]
    async fn external_dialers_fail_over_in_chains() {
        let opts = DialerOptions::default()
            .with_external("ssh", Arc::new(PipeDialer { fail: true }))
            .with_external("ssh2", Arc::new(PipeDialer { fail: false }));
        let mut profiles = external_profiles();
        profiles.insert(
            "ssh2".to_string(),
            ProfileConfig::of_kind(ProfileType::External),
        );
        let reg = ProfileRegistry::from_profiles(&profiles, &opts).unwrap();
        let out = reg
            .chain(&["ssh", "ssh2"])
            .unwrap()
            .dial("a.example", 22, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!((out.profile.as_str(), out.attempt), ("ssh2", 1));
        let err = reg
            .chain(&["ssh"])
            .unwrap()
            .dial("a.example", 22, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
    }

    /// Counts `connect` calls; fails with `ConnectionRefused` while `fail` is set.
    struct CountingDialer {
        fail: std::sync::atomic::AtomicBool,
        calls: AtomicU64,
    }

    impl CountingDialer {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                fail: std::sync::atomic::AtomicBool::new(fail),
                calls: AtomicU64::new(0),
            })
        }

        fn calls(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl UpstreamDialer for CountingDialer {
        fn connect(&self, _host: &str, _port: u16, _timeout: Duration) -> DialFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let fail = self.fail.load(Ordering::SeqCst);
            Box::pin(async move {
                if fail {
                    return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "down"));
                }
                let (near, _far) = tokio::io::duplex(8);
                Ok(UpstreamStream::boxed(near))
            })
        }

        fn redacted_upstream(&self) -> String {
            "external:counting".to_string()
        }
    }

    fn cooldown_registry(
        cooldown: Duration,
        dialers: &[(&str, Arc<CountingDialer>)],
    ) -> ProfileRegistry {
        let mut reg = ProfileRegistry::default().with_failover_cooldown(cooldown);
        for (name, d) in dialers {
            reg.insert(*name, d.clone() as Arc<dyn UpstreamDialer>);
        }
        reg
    }

    async fn dial_profile(reg: &ProfileRegistry, names: &[&str]) -> io::Result<DialOutcome> {
        reg.chain(names)
            .unwrap()
            .dial("a.example", 80, Duration::from_secs(1))
            .await
    }

    #[test]
    fn failover_cooldown_defaults_to_three_seconds() {
        assert_eq!(
            ProfileRegistry::default().failover_cooldown(),
            DEFAULT_FAILOVER_COOLDOWN
        );
        let reg =
            ProfileRegistry::from_profiles(&BTreeMap::new(), &DialerOptions::default()).unwrap();
        assert_eq!(reg.failover_cooldown(), Duration::from_secs(3));
    }

    #[tokio::test]
    async fn failover_cooldown_skips_dead_primary() {
        let (vpn, ssh) = (CountingDialer::new(true), CountingDialer::new(false));
        let reg = cooldown_registry(
            Duration::from_secs(60),
            &[("vpn", vpn.clone()), ("ssh", ssh.clone())],
        );
        let out = dial_profile(&reg, &["vpn", "ssh"]).await.unwrap();
        assert_eq!((out.profile.as_str(), out.attempt), ("ssh", 1));
        assert_eq!(vpn.calls(), 1);
        // During cooldown the dead primary is not dialed at all.
        for _ in 0..3 {
            let out = dial_profile(&reg, &["vpn", "ssh"]).await.unwrap();
            assert_eq!(out.profile, "ssh");
        }
        assert_eq!((vpn.calls(), ssh.calls()), (1, 4));
    }

    #[tokio::test]
    async fn failover_cooldown_probes_primary_after_expiry() {
        let (vpn, ssh) = (CountingDialer::new(true), CountingDialer::new(false));
        let reg = cooldown_registry(
            Duration::from_millis(50),
            &[("vpn", vpn.clone()), ("ssh", ssh.clone())],
        );
        assert_eq!(
            dial_profile(&reg, &["vpn", "ssh"]).await.unwrap().profile,
            "ssh"
        );
        assert_eq!(
            dial_profile(&reg, &["vpn", "ssh"]).await.unwrap().profile,
            "ssh"
        );
        assert_eq!(vpn.calls(), 1);
        vpn.fail.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(80)).await;
        // Cooldown elapsed: the primary is probed and recovers.
        let out = dial_profile(&reg, &["vpn", "ssh"]).await.unwrap();
        assert_eq!((out.profile.as_str(), out.attempt), ("vpn", 0));
        assert_eq!(vpn.calls(), 2);
        // Success cleared the mark: it stays primary immediately afterwards.
        assert_eq!(
            dial_profile(&reg, &["vpn", "ssh"]).await.unwrap().profile,
            "vpn"
        );
        assert_eq!(vpn.calls(), 3);
    }

    #[tokio::test]
    async fn failover_cooldown_never_skips_single_target() {
        let vpn = CountingDialer::new(true);
        let reg = cooldown_registry(Duration::from_secs(60), &[("vpn", vpn.clone())]);
        for _ in 0..3 {
            let err = dial_profile(&reg, &["vpn"]).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
        }
        assert_eq!(vpn.calls(), 3);
    }

    #[tokio::test]
    async fn failover_cooldown_zero_disables_skipping() {
        let (vpn, ssh) = (CountingDialer::new(true), CountingDialer::new(false));
        let reg = cooldown_registry(
            Duration::ZERO,
            &[("vpn", vpn.clone()), ("ssh", ssh.clone())],
        );
        assert_eq!(reg.failover_cooldown(), Duration::ZERO);
        for _ in 0..3 {
            assert_eq!(
                dial_profile(&reg, &["vpn", "ssh"]).await.unwrap().profile,
                "ssh"
            );
        }
        assert_eq!((vpn.calls(), ssh.calls()), (3, 3));
    }

    #[tokio::test]
    async fn failover_cooldown_all_cooling_still_dials() {
        let (vpn, ssh) = (CountingDialer::new(true), CountingDialer::new(true));
        let reg = cooldown_registry(
            Duration::from_secs(60),
            &[("vpn", vpn.clone()), ("ssh", ssh.clone())],
        );
        assert!(dial_profile(&reg, &["vpn", "ssh"]).await.is_err());
        assert_eq!((vpn.calls(), ssh.calls()), (1, 1));
        // Both are cooling down: the chain still dials them in order.
        ssh.fail.store(false, Ordering::SeqCst);
        let out = dial_profile(&reg, &["vpn", "ssh"]).await.unwrap();
        assert_eq!((out.profile.as_str(), out.attempt), ("ssh", 1));
        assert_eq!((vpn.calls(), ssh.calls()), (2, 2));
    }

    #[tokio::test]
    async fn empty_chain_errors() {
        let err = FailoverDialer::new(Vec::new())
            .dial("a", 1, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
