//! Routing and upstream-profile model shared by every rusty-conduit CLI.
//!
//! The serde types here ([`RoutingConfig`], [`RuleConfig`], [`ProfileConfig`], ...) describe
//! the Auto Switch model: named upstream profiles and the routing table (with optional
//! per-inbound split overrides). They are deserialized by the CLIs from their own file
//! formats; this crate never reads configuration files itself.
//!
//! [`RoutingInput`] gathers everything needed to validate and compile that model (profiles,
//! routing table, global destination filter, MITM flag, active listeners, keepalive and
//! connect timeout) without depending on any CLI configuration type. [`RoutingState`] is
//! one compiled, immutable snapshot (router + upstream dialers) and [`SharedRouting`] the
//! lock-free hot-swappable handle the protocol engines read from.
//!
//! [`RoutingCounters`] (reachable as [`RoutingState::counters`]) holds live per-profile and
//! per-rule traffic counters (`active`, `bytes_in`, `bytes_out`, `hits`) updated by the HTTP,
//! MITM, and SOCKS engines.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use dashmap::DashMap;
use serde::Deserialize;

use crate::filter::AccessDenied;
use crate::filter::HostFilter;
use crate::net::KeepaliveSettings;
use crate::router::{RouteDecision, RouteRequest, Router, RouterError};
use crate::upstream::{
    DialerOptions, HttpConnectDialer, ProfileRegistry, TlsOptions, UpstreamError,
};

/// Default per-attempt upstream connect timeout.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Name of the built-in direct-connection profile.
pub const PROFILE_DIRECT: &str = "direct";
/// Name of the built-in reject-everything profile.
pub const PROFILE_BLOCK: &str = "block";

/// Errors raised while validating the routing / profile model. Every variant is `FATAL` at
/// startup (and rejects the candidate configuration on hot-reload).
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("invalid profile `{profile}`: {message}")]
    InvalidProfile { profile: String, message: String },
    #[error(
        "ambiguous password source for profile `{profile}`: use only one of password, password_env, password_file"
    )]
    AmbiguousPassword { profile: String },
    #[error("profile `{profile}`: failed to resolve password: {message}")]
    ResolvePassword { profile: String, message: String },
    #[error("{location} references unknown profile `{target}`")]
    UnknownProfile { location: String, target: String },
    #[error("{location} has an empty failover chain")]
    EmptyTarget { location: String },
    #[error(
        "profile `{profile}` points to this server's own listener {listener} (self-loop); \
         refusing to start"
    )]
    SelfLoop { profile: String, listener: String },
    #[error("invalid routing configuration: {0}")]
    Routing(#[from] RouterError),
}

/// Inbound protocol family (`inbound = "http" | "socks"` rule matcher).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Inbound {
    Http,
    Socks,
}

impl Inbound {
    /// Every inbound family, in display order.
    pub const ALL: [Inbound; 2] = [Inbound::Http, Inbound::Socks];
}

impl fmt::Display for Inbound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Http => "http",
            Self::Socks => "socks",
        })
    }
}

/// Error parsing an [`Inbound`] name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid inbound `{0}` (expected `http` or `socks`)")]
pub struct ParseInboundError(pub String);

impl FromStr for Inbound {
    type Err = ParseInboundError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "http" => Ok(Self::Http),
            "socks" => Ok(Self::Socks),
            other => Err(ParseInboundError(other.to_string())),
        }
    }
}

/// A TOML value that may be written as a single item or as a list (`"a"` or `["a", "b"]`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    /// All values as a slice (one element for the single form).
    pub fn as_slice(&self) -> &[T] {
        match self {
            Self::One(v) => std::slice::from_ref(v),
            Self::Many(v) => v,
        }
    }
}

/// A routing target: one profile name (`"corp"`) or an ordered failover chain
/// (`["corp", "backup", "direct"]`).
pub type Target = OneOrMany<String>;

/// One `port` matcher entry: a number (`443`) or an inclusive range string (`"8000-8080"`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum PortSpec {
    Port(u16),
    Range(String),
}

/// `host_levels = { min = M, max = N }` matcher.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostLevels {
    pub min: Option<u32>,
    pub max: Option<u32>,
}

/// One `[[routing.rules]]` entry, as written in the file. Matching semantics live in the
/// router; here every field is only parsed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleConfig {
    pub name: Option<String>,
    pub inbound: Option<Inbound>,
    pub host: Option<OneOrMany<String>>,
    pub host_wildcard: Option<OneOrMany<String>>,
    pub host_regex: Option<OneOrMany<String>>,
    pub cidr: Option<OneOrMany<String>>,
    pub ip: Option<OneOrMany<String>>,
    pub port: Option<OneOrMany<PortSpec>>,
    pub host_levels: Option<HostLevels>,
    pub bypass: Option<OneOrMany<String>>,
    pub weekday: Option<OneOrMany<String>>,
    pub time: Option<OneOrMany<String>>,
    pub url_wildcard: Option<OneOrMany<String>>,
    pub url_regex: Option<OneOrMany<String>>,
    pub keyword: Option<OneOrMany<String>>,
    /// Matches unconditionally (AirSwitch `TrueCondition`).
    #[serde(default)]
    pub match_all: bool,
    /// Skips the rule (AirSwitch `FalseCondition`).
    #[serde(default)]
    pub disabled: bool,
    /// Per-rule override of `[routing].resolve_dns`.
    pub resolve_dns: Option<bool>,
    pub target: Target,
}

/// `[routing.http]` / `[routing.socks]` split override table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingTableConfig {
    /// Fallback when no rule matches (if absent: `[routing].default` when `inherit`, else `direct`).
    pub default: Option<Target>,
    /// Fall back to the unified `[routing]` table when no override rule matches.
    #[serde(default)]
    pub inherit: bool,
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

/// Unified `[routing]` table plus optional split overrides.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingConfig {
    #[serde(default = "default_target")]
    pub default: Target,
    /// Match hostnames against IP/CIDR rules via local DNS (off by default: no DNS leaks).
    #[serde(default)]
    pub resolve_dns: bool,
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
    pub http: Option<RoutingTableConfig>,
    pub socks: Option<RoutingTableConfig>,
}

fn default_target() -> Target {
    OneOrMany::One(PROFILE_DIRECT.to_string())
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            default: default_target(),
            resolve_dns: false,
            rules: Vec::new(),
            http: None,
            socks: None,
        }
    }
}

impl RoutingConfig {
    /// Total number of rules across the unified and split tables.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
            + self.http.as_ref().map_or(0, |t| t.rules.len())
            + self.socks.as_ref().map_or(0, |t| t.rules.len())
    }

    /// The default target of the table used for `inbound` traffic (no rule evaluation).
    pub fn default_for(&self, inbound: Inbound) -> &Target {
        let split = match inbound {
            Inbound::Http => self.http.as_ref(),
            Inbound::Socks => self.socks.as_ref(),
        };
        match split {
            Some(t) => match (&t.default, t.inherit) {
                (Some(d), _) => d,
                (None, true) => &self.default,
                (None, false) => {
                    static DIRECT: std::sync::OnceLock<Target> = std::sync::OnceLock::new();
                    DIRECT.get_or_init(default_target)
                }
            },
            None => &self.default,
        }
    }
}

/// Upstream profile type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileType {
    Direct,
    Block,
    Http,
    Https,
    Socks4a,
    Socks5,
    /// Per-scheme proxy selection (AirSwitch `FixedProfile`); `fixed` is accepted as an alias.
    #[serde(alias = "fixed")]
    Composite,
    /// Caller-supplied dialer (SSH channel, tunnel, ...), registered through
    /// [`DialerOptions::with_external`] / [`RoutingState::with_external`]. API-only: never
    /// accepted from configuration files.
    #[serde(skip)]
    External,
}

impl ProfileType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Block => "block",
            Self::Http => "http",
            Self::Https => "https",
            Self::Socks4a => "socks4a",
            Self::Socks5 => "socks5",
            Self::Composite => "composite",
            Self::External => "external",
        }
    }

    /// Whether this type dials a remote proxy at `host:port`.
    pub fn is_proxy(self) -> bool {
        matches!(
            self,
            Self::Http | Self::Https | Self::Socks4a | Self::Socks5
        )
    }
}

impl fmt::Display for ProfileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `[profiles.<name>]` table.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    #[serde(rename = "type")]
    pub kind: ProfileType,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub password_env: Option<String>,
    pub password_file: Option<PathBuf>,
    /// Targets matching this list go `direct` instead (AirSwitch `bypassList`).
    #[serde(default)]
    pub bypass: Vec<String>,
    /// `https` only: skip upstream proxy certificate verification (default false).
    #[serde(default)]
    pub tls_insecure_skip_verify: bool,
    /// `composite` only: profile used for `http://` traffic.
    pub http_proxy: Option<String>,
    /// `composite` only: profile used for `https://` traffic.
    pub https_proxy: Option<String>,
    /// `composite` only: profile used for everything else.
    pub fallback_proxy: Option<String>,
}

impl fmt::Debug for ProfileConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileConfig")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("password_env", &self.password_env)
            .field("password_file", &self.password_file)
            .field("bypass", &self.bypass)
            .field("tls_insecure_skip_verify", &self.tls_insecure_skip_verify)
            .field("http_proxy", &self.http_proxy)
            .field("https_proxy", &self.https_proxy)
            .field("fallback_proxy", &self.fallback_proxy)
            .finish()
    }
}

impl ProfileConfig {
    /// A profile of `kind` with every optional field unset.
    pub fn of_kind(kind: ProfileType) -> Self {
        Self {
            kind,
            host: None,
            port: None,
            username: None,
            password: None,
            password_env: None,
            password_file: None,
            bypass: Vec::new(),
            tls_insecure_skip_verify: false,
            http_proxy: None,
            https_proxy: None,
            fallback_proxy: None,
        }
    }

    /// Redacted upstream address for logs and diagnostics (`scheme://host:port`), never
    /// carrying credentials. `direct`/`block`/`composite` render as their type name.
    pub fn redacted_upstream(&self) -> String {
        match (self.kind.is_proxy(), &self.host, self.port) {
            (true, Some(host), Some(port)) => {
                let host = if host.contains(':') && !host.starts_with('[') {
                    format!("[{host}]")
                } else {
                    host.clone()
                };
                format!("{}://{host}:{port}", self.kind)
            }
            _ => self.kind.as_str().to_string(),
        }
    }

    /// Number of configured password sources (`password`, `password_env`, `password_file`).
    pub(crate) fn password_source_count(&self) -> usize {
        [
            self.password.is_some(),
            self.password_env.is_some(),
            self.password_file.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count()
    }

    /// Resolve the password from whichever single source is configured (file contents have
    /// a trailing `\n` / `\r\n` trimmed). Errors never include the secret itself.
    pub fn resolve_password(&self, name: &str) -> Result<Option<String>, RoutingError> {
        let err = |message: String| RoutingError::ResolvePassword {
            profile: name.to_string(),
            message,
        };
        if self.password_source_count() > 1 {
            return Err(RoutingError::AmbiguousPassword {
                profile: name.to_string(),
            });
        }
        if let Some(p) = &self.password {
            return Ok(Some(p.clone()));
        }
        if let Some(var) = &self.password_env {
            return std::env::var(var)
                .map(Some)
                .map_err(|e| err(format!("environment variable {var}: {e}")));
        }
        if let Some(path) = &self.password_file {
            let text = fs::read_to_string(path)
                .map_err(|e| err(format!("password file {}: {e}", path.display())))?;
            return Ok(Some(trim_newline(&text).to_string()));
        }
        Ok(None)
    }
}
// ---------------------------------------------------------------------------
// Routing input
// ---------------------------------------------------------------------------

/// Everything needed to validate and compile the routing model, independent of how a CLI
/// loads its configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingInput {
    /// Declared upstream profiles (built-ins are added by [`Self::profiles_with_builtins`]).
    pub profiles: BTreeMap<String, ProfileConfig>,
    pub routing: RoutingConfig,
    /// Global destination filter (`None` = unfiltered).
    pub filter: Option<HostFilter>,
    /// Whether HTTPS MITM inspection is enabled (affects URL-rule notices only).
    pub mitm_enabled: bool,
    /// Active `(bind, port)` listener addresses, used for self-loop detection.
    pub listeners: Vec<SocketAddr>,
    /// TCP keepalive for upstream sockets (`None` = disabled).
    pub keepalive: Option<KeepaliveSettings>,
    /// Per-attempt upstream connect timeout.
    pub connect_timeout: Duration,
}

impl Default for RoutingInput {
    fn default() -> Self {
        Self {
            profiles: BTreeMap::new(),
            routing: RoutingConfig::default(),
            filter: None,
            mitm_enabled: false,
            listeners: Vec::new(),
            keepalive: Some(KeepaliveSettings::DEFAULT),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }
}

impl RoutingInput {
    /// Whether `name` is a declared or built-in profile.
    pub fn has_profile(&self, name: &str) -> bool {
        name == PROFILE_DIRECT || name == PROFILE_BLOCK || self.profiles.contains_key(name)
    }

    /// Declared profiles plus the built-in `direct` and `block` profiles (unless redeclared).
    pub fn profiles_with_builtins(&self) -> BTreeMap<String, ProfileConfig> {
        let mut all = self.profiles.clone();
        for (name, kind) in [
            (PROFILE_DIRECT, ProfileType::Direct),
            (PROFILE_BLOCK, ProfileType::Block),
        ] {
            all.entry(name.to_string())
                .or_insert_with(|| ProfileConfig::of_kind(kind));
        }
        all
    }

    /// Fully validate the routing model: profile shapes, password sources, profile
    /// references, self-loops against [`Self::listeners`], and every rule, bypass list, and
    /// composite profile exactly as the router will compile them.
    pub fn validate(&self) -> Result<(), RoutingError> {
        self.validate_profiles()?;
        self.validate_targets()?;
        self.validate_no_self_loop()?;
        crate::router::validate::validate_config(self)?;
        Ok(())
    }

    /// Per-profile checks (names, built-in types, password sources, required fields).
    pub fn validate_profiles(&self) -> Result<(), RoutingError> {
        for (name, p) in &self.profiles {
            let invalid = |message: String| RoutingError::InvalidProfile {
                profile: name.clone(),
                message,
            };
            if name.is_empty() {
                return Err(invalid("profile name must not be empty".into()));
            }
            let builtin = match name.as_str() {
                PROFILE_DIRECT => Some(ProfileType::Direct),
                PROFILE_BLOCK => Some(ProfileType::Block),
                _ => None,
            };
            if let Some(kind) = builtin {
                if p.kind != kind {
                    return Err(invalid(format!(
                        "built-in profile `{name}` cannot be redefined as type `{}`",
                        p.kind
                    )));
                }
            }
            if p.password_source_count() > 1 {
                return Err(RoutingError::AmbiguousPassword {
                    profile: name.clone(),
                });
            }
            if p.kind.is_proxy() {
                match p.host.as_deref().map(str::trim) {
                    None | Some("") => {
                        return Err(invalid(format!("type `{}` requires `host`", p.kind)))
                    }
                    Some(_) => {}
                }
                match p.port {
                    None | Some(0) => {
                        return Err(invalid(format!(
                            "type `{}` requires a non-zero `port`",
                            p.kind
                        )))
                    }
                    Some(_) => {}
                }
            }
            if p.kind == ProfileType::Composite
                && p.http_proxy.is_none()
                && p.https_proxy.is_none()
                && p.fallback_proxy.is_none()
            {
                return Err(invalid(
                    "type `composite` requires at least one of http_proxy, https_proxy, fallback_proxy"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// Every routing target and composite sub-profile must name an existing profile.
    pub fn validate_targets(&self) -> Result<(), RoutingError> {
        let check = |location: String, target: &Target| -> Result<(), RoutingError> {
            let names = target.as_slice();
            if names.is_empty() {
                return Err(RoutingError::EmptyTarget { location });
            }
            match names.iter().find(|n| !self.has_profile(n)) {
                Some(missing) => Err(RoutingError::UnknownProfile {
                    location,
                    target: missing.clone(),
                }),
                None => Ok(()),
            }
        };
        let check_rules = |table: &str, rules: &[RuleConfig]| -> Result<(), RoutingError> {
            for (i, rule) in rules.iter().enumerate() {
                let label = match &rule.name {
                    Some(n) => format!("{table} rule #{} ({n:?})", i + 1),
                    None => format!("{table} rule #{}", i + 1),
                };
                check(label, &rule.target)?;
            }
            Ok(())
        };

        check("[routing].default".into(), &self.routing.default)?;
        check_rules("[routing]", &self.routing.rules)?;
        for (key, table) in [("http", &self.routing.http), ("socks", &self.routing.socks)] {
            if let Some(t) = table {
                if let Some(d) = &t.default {
                    check(format!("[routing.{key}].default"), d)?;
                }
                check_rules(&format!("[routing.{key}]"), &t.rules)?;
            }
        }
        for (name, p) in &self.profiles {
            for (field, sub) in [
                ("http_proxy", &p.http_proxy),
                ("https_proxy", &p.https_proxy),
                ("fallback_proxy", &p.fallback_proxy),
            ] {
                if let Some(sub) = sub {
                    if !self.has_profile(sub) {
                        return Err(RoutingError::UnknownProfile {
                            location: format!("profile `{name}` {field}"),
                            target: sub.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Reject any proxy profile whose `host:port` is one of [`Self::listeners`].
    ///
    /// `localhost` counts as `127.0.0.1` / `::1`. When a listener binds an unspecified
    /// address (`0.0.0.0` / `::`), loopback and unspecified upstream addresses also count.
    /// Hostnames other than `localhost` are not resolved (no DNS at validation time).
    pub fn validate_no_self_loop(&self) -> Result<(), RoutingError> {
        for (name, p) in &self.profiles {
            let (true, Some(host), Some(port)) = (p.kind.is_proxy(), &p.host, p.port) else {
                continue;
            };
            if let Some(l) = self
                .listeners
                .iter()
                .find(|l| l.port() == port && host_targets_listener(host, l.ip()))
            {
                return Err(RoutingError::SelfLoop {
                    profile: name.clone(),
                    listener: l.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// Canonicalize IPv4-mapped IPv6 (`::ffff:a.b.c.d`) to IPv4.
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Whether an upstream `host` (IP literal or `localhost`) reaches a listener bound on `bind`.
pub fn host_targets_listener(host: &str, bind: IpAddr) -> bool {
    let host = host.trim().trim_end_matches('.');
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let candidates: Vec<IpAddr> = if host.eq_ignore_ascii_case("localhost") {
        vec![
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ]
    } else {
        match host.parse::<IpAddr>() {
            Ok(ip) => vec![canonical_ip(ip)],
            Err(_) => return false,
        }
    };
    let bind = canonical_ip(bind);
    candidates.into_iter().any(|ip| {
        if bind.is_unspecified() {
            ip.is_loopback() || ip.is_unspecified()
        } else {
            ip == bind
        }
    })
}

/// Strip one trailing `\n` / `\r\n` (secret files).
pub(crate) fn trim_newline(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

// ---------------------------------------------------------------------------
// Compiled routing state
// ---------------------------------------------------------------------------
#[derive(Debug, thiserror::Error)]
pub enum RoutingSetupError {
    #[error(transparent)]
    Router(#[from] RouterError),
    #[error(transparent)]
    Upstream(#[from] UpstreamError),
}

/// One immutable snapshot of everything needed to route and dial a request: the compiled
/// [`Router`], the [`ProfileRegistry`] of tunnel dialers, the `http` / `https` proxy
/// profiles usable for absolute-form plain HTTP forwarding, the global destination filter,
/// timeouts, keepalive, and the generation that produced it. Cheap to clone.
#[derive(Clone)]
pub struct RoutingState {
    pub router: Arc<Router>,
    pub registry: Arc<ProfileRegistry>,
    /// `http` / `https` profiles by name, for absolute-form plain HTTP forwarding.
    pub forward_proxies: Arc<BTreeMap<String, HttpConnectDialer>>,
    /// Per-attempt upstream connect timeout.
    pub connect_timeout: Duration,
    /// TCP keepalive for accepted client sockets and upstream sockets (`None` = disabled).
    pub keepalive: Option<KeepaliveSettings>,
    /// Global destination filter (also compiled into the router and `direct` dialers).
    pub filter: Option<Arc<HostFilter>>,
    /// `0` for the startup configuration, incremented by every successful reload.
    pub config_generation: u64,
    /// Live per-profile / per-rule traffic counters. A fresh state starts with empty
    /// counters; pass the previous snapshot's to [`Self::with_counters`] on reload to keep
    /// them accumulating across generations.
    pub counters: RoutingCounters,
}

impl fmt::Debug for RoutingState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoutingState")
            .field("config_generation", &self.config_generation)
            .field("registry", &self.registry)
            .field(
                "forward_proxies",
                &self.forward_proxies.keys().collect::<Vec<_>>(),
            )
            .field("connect_timeout", &self.connect_timeout)
            .field("keepalive", &self.keepalive)
            .field("filter", &self.filter)
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Traffic counters
// ---------------------------------------------------------------------------

/// Live traffic counters of one profile or rule.
#[derive(Debug, Default)]
pub struct TrafficCounters {
    active: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    hits: AtomicU64,
}

impl TrafficCounters {
    /// A point-in-time copy of the counters.
    pub fn snapshot(&self) -> TrafficSnapshot {
        TrafficSnapshot {
            active: self.active.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            hits: self.hits.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time copy of [`TrafficCounters`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TrafficSnapshot {
    /// Requests / tunnels / SOCKS sessions currently in flight.
    pub active: u64,
    /// Bytes received from clients by finished requests / sessions.
    pub bytes_in: u64,
    /// Bytes sent to clients by finished requests / sessions.
    pub bytes_out: u64,
    /// Requests / tunnels / SOCKS sessions routed so far.
    pub hits: u64,
}

#[derive(Default)]
struct CountersInner {
    profiles: DashMap<String, Arc<TrafficCounters>>,
    rules: DashMap<String, Arc<TrafficCounters>>,
}

/// Per-profile and per-rule [`TrafficCounters`], shared by every clone (cheap to clone).
///
/// Every routed HTTP request, `CONNECT` tunnel, decrypted MITM request, and SOCKS session
/// counts one `hit` and is `active` until it finishes, when its client byte totals are
/// added. Profiles are keyed by the profile that served it (after failover and bypass; the
/// first target's profile when every attempt failed), rules by their `rule=` log label
/// ([`crate::router::DEFAULT_RULE_LABEL`] for table defaults). Requests denied by the
/// destination filter or rejected before routing are not counted.
#[derive(Clone, Default)]
pub struct RoutingCounters {
    inner: Arc<CountersInner>,
}

impl PartialEq for RoutingCounters {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for RoutingCounters {}

impl fmt::Debug for RoutingCounters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoutingCounters")
            .field("profiles", &self.inner.profiles.len())
            .field("rules", &self.inner.rules.len())
            .finish()
    }
}

fn counters_for(map: &DashMap<String, Arc<TrafficCounters>>, key: &str) -> Arc<TrafficCounters> {
    if let Some(c) = map.get(key) {
        return Arc::clone(&c);
    }
    Arc::clone(&map.entry(key.to_string()).or_default())
}

fn snapshots(map: &DashMap<String, Arc<TrafficCounters>>) -> BTreeMap<String, TrafficSnapshot> {
    map.iter()
        .map(|e| (e.key().clone(), e.value().snapshot()))
        .collect()
}

impl RoutingCounters {
    /// Empty counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counters of profile `name`, if it has served anything yet.
    pub fn profile(&self, name: &str) -> Option<TrafficSnapshot> {
        self.inner.profiles.get(name).map(|c| c.snapshot())
    }

    /// Counters of the rule labelled `label`, if it has matched anything yet.
    pub fn rule(&self, label: &str) -> Option<TrafficSnapshot> {
        self.inner.rules.get(label).map(|c| c.snapshot())
    }

    /// Counters of every profile that has served traffic, by name.
    pub fn profiles(&self) -> BTreeMap<String, TrafficSnapshot> {
        snapshots(&self.inner.profiles)
    }

    /// Counters of every rule that has matched, by label.
    pub fn rules(&self) -> BTreeMap<String, TrafficSnapshot> {
        snapshots(&self.inner.rules)
    }

    /// Count one hit for `rule` and `profile` and mark it active until the returned tracker
    /// is dropped (add its byte totals first with [`TrafficTracker::finish`]).
    pub fn track(&self, rule: &str, profile: &str) -> TrafficTracker {
        let rule = counters_for(&self.inner.rules, rule);
        let profile = counters_for(&self.inner.profiles, profile);
        for c in [&rule, &profile] {
            c.hits.fetch_add(1, Ordering::Relaxed);
            c.active.fetch_add(1, Ordering::Relaxed);
        }
        TrafficTracker { rule, profile }
    }
}

/// One in-flight request / session counted by [`RoutingCounters::track`]; it stops being
/// `active` when dropped.
#[derive(Debug)]
pub struct TrafficTracker {
    rule: Arc<TrafficCounters>,
    profile: Arc<TrafficCounters>,
}

impl TrafficTracker {
    /// Add the request's client byte totals and stop counting it as active.
    pub fn finish(self, bytes_in: u64, bytes_out: u64) {
        for c in [&self.rule, &self.profile] {
            c.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
            c.bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
        }
    }
}

impl Drop for TrafficTracker {
    fn drop(&mut self) {
        for c in [&self.rule, &self.profile] {
            c.active.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Runtime state shared by the listeners and every protocol engine; swapped atomically on
/// reload.
pub type SharedRouting = Arc<ArcSwap<RoutingState>>;

/// Decides where a request goes; the HTTP and SOCKS engines dispatch through an
/// `Arc<dyn RouteResolver>`.
///
/// Implemented by [`RoutingState`] (one fixed snapshot), by [`ArcSwap<RoutingState>`] and
/// [`SharedRouting`] (hot-reloadable). Custom resolvers can override decisions while reusing
/// a [`RoutingState`] for dialing.
///
/// Hot reload: the engines call [`Self::pin`] once per client connection (HTTP), per
/// request (SOCKS), or per decrypted request (MITM with URL rules), then route and dial
/// through the pinned view only, so a reload never changes routing mid-connection.
pub trait RouteResolver: Send + Sync + fmt::Debug {
    /// Route one request: destination-filter check plus rule evaluation.
    fn resolve(&self, req: &RouteRequest<'_>) -> Result<RouteDecision, AccessDenied>;

    /// The snapshot that dials decisions from [`Self::resolve`]: upstream registry,
    /// absolute-form forward proxies, connect timeout, keepalive, and generation.
    fn state(&self) -> Arc<RoutingState>;

    /// A view whose [`Self::resolve`] and [`Self::state`] stay consistent with each other
    /// and fixed from now on (for a hot-reloadable resolver: the current snapshot).
    fn pin(&self) -> Arc<dyn RouteResolver>;
}

impl RouteResolver for RoutingState {
    fn resolve(&self, req: &RouteRequest<'_>) -> Result<RouteDecision, AccessDenied> {
        self.router.route(req)
    }

    fn state(&self) -> Arc<RoutingState> {
        Arc::new(self.clone())
    }

    fn pin(&self) -> Arc<dyn RouteResolver> {
        Arc::new(self.clone())
    }
}

impl RouteResolver for ArcSwap<RoutingState> {
    fn resolve(&self, req: &RouteRequest<'_>) -> Result<RouteDecision, AccessDenied> {
        self.load().router.route(req)
    }

    fn state(&self) -> Arc<RoutingState> {
        self.load_full()
    }

    fn pin(&self) -> Arc<dyn RouteResolver> {
        Arc::new(PinnedState(self.load_full()))
    }
}

impl RouteResolver for SharedRouting {
    fn resolve(&self, req: &RouteRequest<'_>) -> Result<RouteDecision, AccessDenied> {
        self.as_ref().resolve(req)
    }

    fn state(&self) -> Arc<RoutingState> {
        self.as_ref().state()
    }

    fn pin(&self) -> Arc<dyn RouteResolver> {
        self.as_ref().pin()
    }
}

/// One loaded snapshot of a hot-reloadable resolver.
#[derive(Debug)]
struct PinnedState(Arc<RoutingState>);

impl RouteResolver for PinnedState {
    fn resolve(&self, req: &RouteRequest<'_>) -> Result<RouteDecision, AccessDenied> {
        self.0.router.route(req)
    }

    fn state(&self) -> Arc<RoutingState> {
        Arc::clone(&self.0)
    }

    fn pin(&self) -> Arc<dyn RouteResolver> {
        Arc::new(PinnedState(Arc::clone(&self.0)))
    }
}

impl RoutingState {
    /// A state with no absolute-form forward proxies, the default connect timeout, default
    /// keepalive, and no filter.
    pub fn new(router: Router, registry: ProfileRegistry) -> Self {
        Self {
            router: Arc::new(router),
            registry: Arc::new(registry),
            forward_proxies: Arc::new(BTreeMap::new()),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            keepalive: Some(KeepaliveSettings::DEFAULT),
            filter: None,
            config_generation: 0,
            counters: RoutingCounters::new(),
        }
    }

    /// Compile the router and every upstream dialer from a validated [`RoutingInput`]. The
    /// global destination filter (if any) is enforced by the router on the target name and
    /// by `direct` dials on every resolved IP.
    pub fn from_input(
        input: &RoutingInput,
        opts: DialerOptions,
    ) -> Result<Self, RoutingSetupError> {
        let router = Router::from_input(input)?;
        let opts = DialerOptions {
            keepalive: input.keepalive,
            filter: opts.filter.or_else(|| input.filter.clone().map(Arc::new)),
            ..opts
        };
        let profiles = input.profiles_with_builtins();
        let registry = ProfileRegistry::from_profiles(&profiles, &opts)?;
        let mut forward = BTreeMap::new();
        for (name, profile) in &profiles {
            if let Some(d) = forward_proxy_for_profile(name, profile, &opts)? {
                forward.insert(name.clone(), d);
            }
        }
        Ok(Self {
            router: Arc::new(router),
            registry: Arc::new(registry),
            forward_proxies: Arc::new(forward),
            connect_timeout: input.connect_timeout,
            keepalive: opts.keepalive,
            filter: opts.filter,
            config_generation: 0,
            counters: RoutingCounters::new(),
        })
    }

    /// Register `dialer` as the absolute-form forward proxy for profile `name`.
    pub fn with_forward_proxy(
        mut self,
        name: impl Into<String>,
        dialer: HttpConnectDialer,
    ) -> Self {
        Arc::make_mut(&mut self.forward_proxies).insert(name.into(), dialer);
        self
    }

    /// Register (or replace) the dialer for the `external` profile `name` in this
    /// snapshot. Prefer [`DialerOptions::with_external`] with [`Self::from_input`], which
    /// also validates that every declared `external` profile has a dialer.
    pub fn with_external(
        mut self,
        name: impl Into<String>,
        dialer: Arc<dyn crate::upstream::UpstreamDialer>,
    ) -> Self {
        Arc::make_mut(&mut self.registry).insert(name, dialer);
        self
    }

    /// Override the per-attempt upstream connect timeout.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set the generation number of this snapshot.
    pub fn with_generation(mut self, generation: u64) -> Self {
        self.config_generation = generation;
        self
    }

    /// Share `counters` (e.g. the previous snapshot's on reload) instead of fresh ones.
    pub fn with_counters(mut self, counters: RoutingCounters) -> Self {
        self.counters = counters;
        self
    }

    /// Live traffic counters of this snapshot.
    pub fn counters(&self) -> &RoutingCounters {
        &self.counters
    }

    /// Current counters of profile `name` (`None` until it has served traffic).
    pub fn profile_traffic(&self, name: &str) -> Option<TrafficSnapshot> {
        self.counters.profile(name)
    }

    /// Current counters of the rule labelled `label` (`None` until it has matched).
    pub fn rule_traffic(&self, label: &str) -> Option<TrafficSnapshot> {
        self.counters.rule(label)
    }

    /// Profile name counted for `decision` when `attempt` served it, or the first target's
    /// when no attempt succeeded.
    pub(crate) fn counted_profile(decision: &RouteDecision, attempt: Option<usize>) -> String {
        attempt
            .and_then(|i| decision.targets.get(i))
            .or_else(|| decision.targets.first())
            .map(|t| t.profile.clone())
            .unwrap_or_default()
    }

    /// Wrap into a [`SharedRouting`] handle.
    pub fn shared(self) -> SharedRouting {
        Arc::new(ArcSwap::from_pointee(self))
    }

    /// Redacted upstream of the first target of `decision` (for logs when every dial
    /// failed or the route was refused).
    pub fn first_upstream(&self, decision: &RouteDecision) -> String {
        decision
            .targets
            .first()
            .and_then(|t| self.registry.get(&t.profile))
            .map(|d| d.redacted_upstream())
            .unwrap_or_default()
    }
}

/// Build the absolute-form forward proxy dialer for an `http` / `https` profile (`None` for
/// every other type). Mirrors [`crate::upstream::dialer_for_profile`].
fn forward_proxy_for_profile(
    name: &str,
    profile: &ProfileConfig,
    opts: &DialerOptions,
) -> Result<Option<HttpConnectDialer>, UpstreamError> {
    if !matches!(profile.kind, ProfileType::Http | ProfileType::Https) {
        return Ok(None);
    }
    let (Some(host), Some(port)) = (&profile.host, profile.port) else {
        return Err(UpstreamError::Profile {
            profile: name.to_string(),
            message: format!("type `{}` requires host and port", profile.kind),
        });
    };
    let password = profile.resolve_password(name)?;
    let credentials = match (&profile.username, password) {
        (Some(u), p) => Some((u.clone(), p.unwrap_or_default())),
        (None, Some(p)) => Some((String::new(), p)),
        (None, None) => None,
    };
    let dialer = if profile.kind == ProfileType::Https {
        let tls = TlsOptions {
            insecure_skip_verify: profile.tls_insecure_skip_verify,
            roots: opts.tls_roots.clone(),
        };
        HttpConnectDialer::https(host.clone(), port, &tls).map_err(|e| UpstreamError::Profile {
            profile: name.to_string(),
            message: format!("TLS setup failed: {e}"),
        })?
    } else {
        HttpConnectDialer::http(host.clone(), port)
    };
    Ok(Some(
        dialer
            .with_credentials(credentials)
            .with_tcp_keepalive(opts.keepalive),
    ))
}

/// Historical name of [`RoutingState`] (one immutable routing snapshot).
pub type RuntimeConfig = RoutingState;
/// Historical name of [`SharedRouting`].
pub type SharedRuntime = SharedRouting;

#[cfg(test)]
pub(crate) mod test_support {
    //! Build a [`RoutingInput`] from a TOML snippet in unit tests (mirrors how the CLIs
    //! deserialize `[profiles.*]` and `[routing]`, plus the listener keys that drive
    //! self-loop detection).

    use super::*;

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TestFile {
        bind: Option<IpAddr>,
        http_port: Option<u16>,
        socks_port: Option<u16>,
        #[serde(default)]
        filter: TestFilter,
        #[serde(default)]
        profiles: BTreeMap<String, ProfileConfig>,
        #[serde(default)]
        routing: RoutingConfig,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TestFilter {
        #[serde(default)]
        allow: Vec<String>,
        #[serde(default)]
        block: Vec<String>,
    }

    fn rules(raw: &[String]) -> Vec<crate::filter::FilterRule> {
        raw.iter().map(|s| s.parse().unwrap()).collect()
    }

    /// Parse `text` into a [`RoutingInput`] whose listeners default to `127.0.0.1:3128`
    /// (HTTP) and `127.0.0.1:1080` (SOCKS); a port of `0` disables that listener. A
    /// `[filter]` table with `allow` / `block` lists sets the global destination filter.
    pub(crate) fn input(text: &str) -> RoutingInput {
        let file: TestFile = toml::from_str(text).unwrap();
        let bind = file.bind.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let mut ports: Vec<u16> = [
            file.http_port.unwrap_or(3128),
            file.socks_port.unwrap_or(1080),
        ]
        .into_iter()
        .filter(|p| *p > 0)
        .collect();
        ports.dedup();
        let filter = if !file.filter.allow.is_empty() {
            Some(HostFilter::Allow(rules(&file.filter.allow)))
        } else if !file.filter.block.is_empty() {
            Some(HostFilter::Block(rules(&file.filter.block)))
        } else {
            None
        };
        RoutingInput {
            profiles: file.profiles,
            filter,
            routing: file.routing,
            listeners: ports
                .into_iter()
                .map(|p| SocketAddr::new(bind, p))
                .collect(),
            ..RoutingInput::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_parses_and_displays() {
        for i in Inbound::ALL {
            assert_eq!(i.to_string().parse::<Inbound>().unwrap(), i);
        }
        assert!("ftp".parse::<Inbound>().is_err());
    }

    #[test]
    fn input_validation_reports_profile_and_target_errors() {
        let ok = test_support::input("[profiles.p]\ntype = \"socks5\"\nhost = \"10.0.0.1\"\nport = 1080\n[routing]\ndefault = \"p\"\n");
        assert!(ok.validate().is_ok());
        let missing = test_support::input("[routing]\ndefault = \"ghost\"\n");
        assert!(matches!(
            missing.validate(),
            Err(RoutingError::UnknownProfile { .. })
        ));
        let no_host = test_support::input("[profiles.p]\ntype = \"http\"\nport = 1\n");
        assert!(no_host
            .validate()
            .unwrap_err()
            .to_string()
            .contains("requires `host`"));
        let looped = test_support::input(
            "[profiles.p]\ntype = \"http\"\nhost = \"localhost\"\nport = 3128\n",
        );
        assert!(matches!(
            looped.validate(),
            Err(RoutingError::SelfLoop { .. })
        ));
        let builtins = RoutingInput::default().profiles_with_builtins();
        assert!(builtins.contains_key(PROFILE_DIRECT) && builtins.contains_key(PROFILE_BLOCK));
    }

    #[derive(Debug)]
    struct NullDialer;

    impl crate::upstream::UpstreamDialer for NullDialer {
        fn connect(
            &self,
            _host: &str,
            _port: u16,
            _timeout: Duration,
        ) -> crate::upstream::DialFuture<'_> {
            Box::pin(async { Err(std::io::Error::other("null")) })
        }

        fn redacted_upstream(&self) -> String {
            "external:null".into()
        }
    }

    fn external_input() -> RoutingInput {
        let mut input = test_support::input("[routing]\ndefault = [\"ssh\", \"direct\"]\n");
        input
            .profiles
            .insert("ssh".into(), ProfileConfig::of_kind(ProfileType::External));
        input
    }

    #[test]
    fn external_profile_type_is_api_only() {
        let err = toml::from_str::<ProfileConfig>("type = \"external\"\n").unwrap_err();
        assert!(err.to_string().contains("external"), "{err}");
        assert_eq!(ProfileType::External.to_string(), "external");
        assert!(!ProfileType::External.is_proxy());
    }

    #[test]
    fn routing_state_requires_declared_external_dialers() {
        let input = external_input();
        input.validate().unwrap();
        let err = RoutingState::from_input(&input, DialerOptions::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("`ssh`") && err.contains("external"), "{err}");

        let opts = DialerOptions::default().with_external("ssh", Arc::new(NullDialer));
        let state = RoutingState::from_input(&input, opts).unwrap();
        assert_eq!(
            state.registry.get("ssh").unwrap().redacted_upstream(),
            "external:null"
        );
        let route = crate::router::RouteRequest {
            inbound: Inbound::Socks,
            host: "a.example",
            port: 22,
            scheme: None,
            url: None,
        };
        let d = state.router.route(&route).unwrap();
        let chain = state.registry.chain_for(&d.targets).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(d.targets[0].kind, ProfileType::External);

        // An undeclared name is still an unknown profile, even with a dialer supplied.
        let mut undeclared = test_support::input("[routing]\ndefault = \"ghost\"\n");
        undeclared.profiles.clear();
        assert!(matches!(
            undeclared.validate(),
            Err(RoutingError::UnknownProfile { .. })
        ));

        // States built by hand take external dialers through the builder.
        let state = RoutingState::new(
            crate::router::Router::from_input(&RoutingInput::default()).unwrap(),
            ProfileRegistry::default(),
        )
        .with_external("ssh", Arc::new(NullDialer));
        assert!(state.registry.get("ssh").is_some());
    }
}
