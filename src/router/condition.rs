//! Compiled rule matchers.
//!
//! Each `[[routing.rules]]` entry compiles into a [`RuleMatcher`]: a list of [`Condition`]s
//! that must **all** match (logical AND across fields), where each condition matches if
//! **any** of its values matches (logical OR within a field).
//!
//! Wildcard semantics follow AirSwitch / SwitchyOmega (`omega-pac/src/conditions.coffee`):
//!
//! * `host_wildcard`: `*.example.com` matches the apex `example.com` **and** its subdomains,
//!   `**.example.com` matches subdomains only, `*` matches any run of characters and `?` a
//!   single character. A leading `.` is shorthand for `*.`.
//! * `bypass`: Chromium bypass-list syntax — `<local>` (`127.0.0.1`, `::1`, and dot-less
//!   hosts), `[scheme://]host-wildcard[:port]`, IP literals, and `ip/prefix` CIDRs. Here
//!   `*.example.com` matches subdomains only (shell-wildcard semantics, like AirSwitch).
//! * `url_wildcard`: shell wildcard against the full URL; leading and trailing `*` are
//!   implicit (AirSwitch `trimAsterisk`).
//!
//! IP/CIDR conditions never perform DNS lookups unless `resolve_dns` is enabled for the rule
//! (or globally); even then the lookup happens lazily, at most once per routing decision, and
//! only when a CIDR condition is actually evaluated against a hostname.

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "routing")]
use regex::{Regex, RegexBuilder};

#[cfg(not(feature = "routing"))]
pub use no_regex::Regex;

/// Stand-in for `regex` when the default-on `routing` feature is disabled.
///
/// Regex-based conditions (`host_wildcard`, `host_regex`, `url_wildcard`, `url_regex`, and
/// wildcard `bypass` hosts) then fail to compile with a [`ConditionError`] naming the
/// feature, so [`Regex`] is uninhabited and never matches. Exact-host, CIDR, port,
/// host-level, weekday, time, keyword, and inbound conditions keep working.
#[cfg(not(feature = "routing"))]
mod no_regex {
    /// Uninhabited placeholder for `regex::Regex` (no value can exist).
    #[derive(Debug, Clone)]
    pub enum Regex {}

    impl Regex {
        /// Never called: no `Regex` value can be constructed.
        pub fn is_match(&self, _haystack: &str) -> bool {
            match *self {}
        }
    }

    /// Same escaping as `regex::escape` (the `regex-syntax` meta characters).
    pub(super) fn escape(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            if matches!(
                c,
                '\\' | '.'
                    | '+'
                    | '*'
                    | '?'
                    | '('
                    | ')'
                    | '|'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '^'
                    | '$'
                    | '#'
                    | '&'
                    | '-'
                    | '~'
            ) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }
}

#[cfg(not(feature = "routing"))]
use no_regex::escape as regex_escape;
#[cfg(feature = "routing")]
use regex::escape as regex_escape;

use crate::routing::{HostLevels, Inbound, OneOrMany, PortSpec, RuleConfig};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A rule field that failed to compile (bad regex, CIDR, port range, time window, ...).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{field}`: {message}")]
pub struct ConditionError {
    pub field: &'static str,
    pub message: String,
}

impl ConditionError {
    fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Environment: local clock and (opt-in) DNS resolver
// ---------------------------------------------------------------------------

/// Local wall-clock time as seen by `weekday` / `time` conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    /// Day of week, `0` = Monday ... `6` = Sunday.
    pub weekday: u8,
    /// Minutes since local midnight (`0..1440`).
    pub minute_of_day: u16,
}

impl LocalTime {
    /// Build from a weekday (`0` = Monday) and `hour:minute`.
    pub fn new(weekday: u8, hour: u8, minute: u8) -> Self {
        Self {
            weekday: weekday % 7,
            minute_of_day: u16::from(hour % 24) * 60 + u16::from(minute % 60),
        }
    }

    /// Current local time; falls back to UTC if the local offset cannot be determined.
    pub fn now() -> Self {
        let now =
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        Self::new(
            now.weekday().number_days_from_monday(),
            now.hour(),
            now.minute(),
        )
    }
}

/// Source of the local time for `weekday` / `time` conditions (injectable for tests).
pub trait Clock: Send + Sync {
    fn now(&self) -> LocalTime;
}

/// The system clock in the local timezone.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

/// `SystemClock`'s last answer, packed as `unix_minute << 14 | weekday << 11 | minute_of_day`
/// (`0` = none yet). Local time changes only at minute boundaries, so the value for the
/// current unix minute is exact; it only saves the `now_local()` call (glibc's
/// `localtime_r` takes a process-wide lock) for every request evaluated in that minute.
static LOCAL_MINUTE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
static LOCAL_TIME_QUERIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Clock for SystemClock {
    fn now(&self) -> LocalTime {
        let minute = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 60);
        let cached = LOCAL_MINUTE.load(Ordering::Relaxed);
        if cached != 0 && cached >> 14 == minute {
            return LocalTime {
                weekday: ((cached >> 11) & 0x7) as u8,
                minute_of_day: (cached & 0x7ff) as u16,
            };
        }
        #[cfg(test)]
        LOCAL_TIME_QUERIES.fetch_add(1, Ordering::Relaxed);
        let now = LocalTime::now();
        let packed = minute << 14 | u64::from(now.weekday) << 11 | u64::from(now.minute_of_day);
        LOCAL_MINUTE.store(packed, Ordering::Relaxed);
        now
    }
}

/// A clock frozen at one instant.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub LocalTime);

impl Clock for FixedClock {
    fn now(&self) -> LocalTime {
        self.0
    }
}

/// Local DNS resolver used **only** by CIDR/IP conditions with `resolve_dns = true`.
pub trait Resolver: Send + Sync {
    /// Resolve `host` to its addresses; an empty list on failure.
    fn resolve(&self, host: &str, port: u16) -> Vec<IpAddr>;
}

/// The OS resolver (blocking `getaddrinfo`), without any caching.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> Vec<IpAddr> {
        (host, port)
            .to_socket_addrs()
            .map(|addrs| addrs.map(|a| canonical_ip(a.ip())).collect())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Match context
// ---------------------------------------------------------------------------

/// Everything a condition can look at for one routing decision.
pub struct MatchContext<'a> {
    pub inbound: Inbound,
    /// Normalized host: lowercase, no brackets, no trailing dot.
    pub host: &'a str,
    /// The host as a canonical IP literal (IPv4-mapped IPv6 folded to IPv4), if it is one.
    pub ip: Option<IpAddr>,
    pub port: u16,
    /// Lowercase URL scheme (`http` / `https`) when known.
    pub scheme: Option<&'a str>,
    /// Full URL when visible (plain HTTP or decrypted MITM HTTPS).
    pub url: Option<&'a str>,
    clock: &'a dyn Clock,
    resolver: &'a dyn Resolver,
    now: OnceCell<LocalTime>,
    resolved: OnceCell<Vec<IpAddr>>,
}

impl<'a> MatchContext<'a> {
    /// Build a context; `host` must already be normalized (see [`normalize_host`]).
    pub fn new(
        inbound: Inbound,
        host: &'a str,
        port: u16,
        scheme: Option<&'a str>,
        url: Option<&'a str>,
        clock: &'a dyn Clock,
        resolver: &'a dyn Resolver,
    ) -> Self {
        Self {
            inbound,
            host,
            ip: parse_ip_literal(host),
            port,
            scheme,
            url,
            clock,
            resolver,
            now: OnceCell::new(),
            resolved: OnceCell::new(),
        }
    }

    fn now(&self) -> LocalTime {
        *self.now.get_or_init(|| self.clock.now())
    }

    /// Addresses of a hostname via the opt-in resolver (looked up at most once).
    fn resolved(&self) -> &[IpAddr] {
        self.resolved
            .get_or_init(|| self.resolver.resolve(self.host, self.port))
    }
}

/// [`normalize_host`] that borrows `host` when it is already normal (the common case).
pub(crate) fn normalize_host_cow(host: &str) -> Cow<'_, str> {
    let trimmed = host.trim();
    let bare = trimmed
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(trimmed);
    let bare = bare.strip_suffix('.').unwrap_or(bare);
    if bare.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(bare.to_ascii_lowercase())
    } else {
        Cow::Borrowed(bare)
    }
}

/// Normalize a host for matching: trim, strip `[]` brackets and one trailing dot, lowercase.
pub fn normalize_host(host: &str) -> String {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let host = host.strip_suffix('.').unwrap_or(host);
    host.to_ascii_lowercase()
}

/// Canonicalize IPv4-mapped IPv6 (`::ffff:a.b.c.d`) to IPv4.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Parse an IP literal (brackets allowed), canonicalized.
pub fn parse_ip_literal(s: &str) -> Option<IpAddr> {
    let s = s
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(s);
    s.parse::<IpAddr>().ok().map(canonical_ip)
}

// ---------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------

/// An IPv4 or IPv6 network (`addr/prefix`, host bits cleared).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpNet {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl IpNet {
    /// Parse `ip`, `ip/prefix`, or `[ipv6]/prefix`. IPv4-mapped IPv6 networks with a prefix
    /// of at least 96 are canonicalized to the equivalent IPv4 network.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (ip_part, prefix_part) = match s.split_once('/') {
            Some((ip, p)) => (ip, Some(p)),
            None => (s, None),
        };
        let raw_ip = ip_part
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(ip_part)
            .parse::<IpAddr>()
            .map_err(|_| format!("invalid IP address {ip_part:?}"))?;
        let max = if raw_ip.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix_part {
            Some(p) => p
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("invalid prefix length {p:?} for {ip_part} (0-{max})"))?,
            None => max,
        };
        let (addr, prefix) = match raw_ip {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) if prefix >= 96 => (IpAddr::V4(v4), prefix - 96),
                _ => (raw_ip, prefix),
            },
            v4 => (v4, prefix),
        };
        Ok(Self {
            addr: mask(addr, prefix),
            prefix,
        })
    }

    /// Whether `ip` (canonicalized) lies in this network. Families never cross-match.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = canonical_ip(ip);
        match (self.addr, ip) {
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {
                mask(ip, self.prefix) == self.addr
            }
            _ => false,
        }
    }
}

impl fmt::Display for IpNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

fn mask(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let m = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix))
            };
            IpAddr::V4(Ipv4Addr::from(bits & m))
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let m = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(prefix))
            };
            IpAddr::V6(Ipv6Addr::from(bits & m))
        }
    }
}

/// AirSwitch `shExp2RegExp`: translate a shell expression (`*`, `?`) into a regex source.
/// With `trim_asterisk`, leading/trailing `*` are dropped together with the matching anchor.
pub fn shexp_to_regex(pattern: &str, trim_asterisk: bool) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let (mut start, mut end) = (0, chars.len());
    if trim_asterisk {
        while start < end && chars[start] == '*' {
            start += 1;
        }
        while start < end && chars[end - 1] == '*' {
            end -= 1;
        }
    }
    let mut regex = String::new();
    if start == 0 {
        regex.push('^');
    }
    for c in &chars[start..end] {
        match c {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            c => regex.push_str(&regex_escape(&c.to_string())),
        }
    }
    if end == chars.len() {
        regex.push('$');
    }
    regex
}

/// Regex source of one AirSwitch `HostWildcardCondition` pattern.
fn host_wildcard_regex(pattern: &str) -> String {
    let pattern = if pattern.starts_with('.') {
        format!("*{pattern}")
    } else {
        pattern.to_string()
    };
    if let Some(rest) = pattern.strip_prefix("**.") {
        // Subdomains only: `\.example\.com$`.
        shexp_to_regex(&format!("*.{rest}"), true)
    } else if let Some(rest) = pattern.strip_prefix("*.") {
        // Apex + subdomains: `(?:^|\.)example\.com$`.
        let re = shexp_to_regex(rest, false);
        let re = format!("(?:^|\\.){}", &re[1..]);
        // AirSwitch drops a trailing `.*$` so `*.example.*` stays unanchored at the end.
        re.strip_suffix(".*$").map(str::to_string).unwrap_or(re)
    } else {
        shexp_to_regex(&pattern, true)
    }
}

#[cfg(not(feature = "routing"))]
fn build_regex(
    field: &'static str,
    source: &str,
    _case_insensitive: bool,
) -> Result<Regex, ConditionError> {
    Err(ConditionError::new(
        field,
        format!(
            "pattern {source:?} needs regex matching, which requires the `routing` cargo feature"
        ),
    ))
}

#[cfg(feature = "routing")]
fn build_regex(
    field: &'static str,
    source: &str,
    case_insensitive: bool,
) -> Result<Regex, ConditionError> {
    RegexBuilder::new(source)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|e| ConditionError::new(field, format!("invalid pattern {source:?}: {e}")))
}

/// Combine alternatives into one regex (`(?:a)|(?:b)`).
fn alternation(
    field: &'static str,
    sources: &[String],
    case_insensitive: bool,
) -> Result<Regex, ConditionError> {
    let joined = sources
        .iter()
        .map(|s| format!("(?:{s})"))
        .collect::<Vec<_>>()
        .join("|");
    build_regex(field, &joined, case_insensitive)
}

/// Build one regex per `sources` entry, or (when they combine) a single alternation that
/// matches exactly when one of them does. Falls back to separate regexes when the combined
/// pattern cannot be built (for example the compiled-size limit), so errors and results stay
/// those of the individual patterns.
fn regex_list(
    field: &'static str,
    sources: &[String],
    case_insensitive: bool,
) -> Result<Vec<Regex>, ConditionError> {
    let each = sources
        .iter()
        .map(|s| build_regex(field, s, case_insensitive))
        .collect::<Result<Vec<_>, _>>()?;
    if each.len() < 2 {
        return Ok(each);
    }
    Ok(match alternation(field, sources, case_insensitive) {
        Ok(one) => vec![one],
        Err(_) => each,
    })
}

/// Lists up to this long are scanned; longer ones are hashed (hashing per rule costs more
/// than a few direct comparisons, which usually fail on the first bytes).
const SMALL_SET: usize = 8;

/// A set of lowercase names: scanned when small, hashed when large.
#[derive(Debug, Clone)]
pub enum NameSet {
    Few(Vec<String>),
    /// Boxed so a small set stays as compact as a plain `Vec`.
    Many(Box<HashSet<String>>),
}

impl NameSet {
    fn from_names(mut names: Vec<String>) -> Self {
        names.sort();
        names.dedup();
        if names.len() <= SMALL_SET {
            Self::Few(names)
        } else {
            Self::Many(Box::new(names.into_iter().collect()))
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Few(v) => v.is_empty(),
            Self::Many(s) => s.is_empty(),
        }
    }

    /// The names, in no particular order.
    pub(crate) fn iter(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::Few(v) => Box::new(v.iter().map(String::as_str)),
            Self::Many(s) => Box::new(s.iter().map(String::as_str)),
        }
    }

    /// Whether `name` is in the set.
    pub fn contains(&self, name: &str) -> bool {
        match self {
            Self::Few(v) => v.iter().any(|n| n == name),
            Self::Many(s) => s.contains(name),
        }
    }

    /// Whether `host` ends with `.d` for some `d` in the set (with `apex`, also `host == d`).
    fn has_suffix_of(&self, host: &str, apex: bool) -> bool {
        match self {
            Self::Few(v) => v.iter().any(|d| {
                (apex && host == d)
                    || host
                        .strip_suffix(d.as_str())
                        .is_some_and(|rest| rest.ends_with('.'))
            }),
            Self::Many(s) => {
                (apex && s.contains(host))
                    || host
                        .bytes()
                        .enumerate()
                        .any(|(i, b)| b == b'.' && s.contains(&host[i + 1..]))
            }
        }
    }
}

/// A compiled `host_wildcard` field. Plain-name, `*.domain`, `**.domain`, and `*` patterns
/// are answered from hash sets by walking the host's label suffixes; every other pattern
/// stays in a residual regex. Non-ASCII hosts always use the full regex, because its
/// case-insensitive Unicode matching can fold characters (`K` → `k`) that the sets do not.
#[derive(Debug, Clone)]
pub struct HostWildcardMatcher {
    /// The whole field as one regex (the reference semantics).
    full: Regex,
    any: bool,
    exact: NameSet,
    /// `*.d`: `d` and its subdomains.
    apex_and_subdomains: NameSet,
    /// `**.d`: subdomains of `d` only.
    subdomains: NameSet,
    residual: Option<Regex>,
}

impl HostWildcardMatcher {
    fn compile(patterns: &[String]) -> Result<Self, ConditionError> {
        let sources: Vec<String> = patterns.iter().map(|p| host_wildcard_regex(p)).collect();
        // Compiled first, so errors (and the no-`routing` rejection) are unchanged.
        let full = alternation("host_wildcard", &sources, true)?;
        let mut any = false;
        let (mut exact, mut apex, mut sub) = (Vec::new(), Vec::new(), Vec::new());
        let plain = |s: &str| !s.is_empty() && s.is_ascii() && !s.contains(['*', '?']);
        let mut rest = Vec::new();
        for (pattern, source) in patterns.iter().zip(sources) {
            let p = if pattern.starts_with('.') {
                format!("*{pattern}")
            } else {
                pattern.clone()
            };
            if p == "*" {
                any = true;
            } else if let Some(d) = p.strip_prefix("**.").filter(|d| plain(d)) {
                sub.push(d.to_ascii_lowercase());
            } else if let Some(d) = p.strip_prefix("*.").filter(|d| plain(d)) {
                apex.push(d.to_ascii_lowercase());
            } else if plain(&p) {
                exact.push(p.to_ascii_lowercase());
            } else {
                rest.push(source);
            }
        }
        let residual = if rest.is_empty() {
            None
        } else {
            Some(alternation("host_wildcard", &rest, true)?)
        };
        Ok(Self {
            full,
            any,
            exact: NameSet::from_names(exact),
            apex_and_subdomains: NameSet::from_names(apex),
            subdomains: NameSet::from_names(sub),
            residual,
        })
    }

    /// Whether the (normalized) host matches any pattern.
    pub fn is_match(&self, host: &str) -> bool {
        if !host.is_ascii() {
            return self.full.is_match(host);
        }
        if self.any {
            return true;
        }
        let host_lower;
        let host = if host.bytes().any(|b| b.is_ascii_uppercase()) {
            host_lower = host.to_ascii_lowercase();
            host_lower.as_str()
        } else {
            host
        };
        if self.exact.contains(host)
            || (!self.apex_and_subdomains.is_empty()
                && self.apex_and_subdomains.has_suffix_of(host, true))
            || (!self.subdomains.is_empty() && self.subdomains.has_suffix_of(host, false))
        {
            return true;
        }
        self.residual.as_ref().is_some_and(|re| re.is_match(host))
    }

    /// The reference regex for the whole field (tests compare against it).
    #[cfg(all(test, feature = "routing"))]
    fn full_is_match(&self, host: &str) -> bool {
        self.full.is_match(host)
    }
}

/// Values of a list field, rejecting an explicitly empty list.
fn values<'r, T>(field: &'static str, v: &'r OneOrMany<T>) -> Result<&'r [T], ConditionError> {
    let s = v.as_slice();
    if s.is_empty() {
        return Err(ConditionError::new(field, "must list at least one value"));
    }
    Ok(s)
}

/// Split `|`-delimited patterns (AirSwitch style) and drop empty pieces.
fn split_patterns(field: &'static str, items: &[String]) -> Result<Vec<String>, ConditionError> {
    let out: Vec<String> = items
        .iter()
        .flat_map(|s| s.split('|'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if out.is_empty() {
        return Err(ConditionError::new(field, "contains no non-empty pattern"));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Bypass patterns
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum BypassHost {
    Any,
    Ip(IpAddr),
    Cidr(IpNet),
    Wildcard(Regex),
}

/// One compiled Chromium / AirSwitch bypass-list entry.
#[derive(Debug, Clone)]
pub enum BypassPattern {
    /// `<local>`: `127.0.0.1`, `::1`, or any host without a dot.
    Local,
    Entry {
        scheme: Option<String>,
        host: BypassHost,
        port: Option<u16>,
    },
}

impl BypassPattern {
    /// Parse one bypass entry.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let pattern = raw.trim();
        if pattern.is_empty() {
            return Err("empty bypass pattern".into());
        }
        if pattern.eq_ignore_ascii_case("<local>") {
            return Ok(Self::Local);
        }
        let (scheme, mut server) = match pattern.split_once("://") {
            Some((s, rest)) => (Some(s.to_ascii_lowercase()), rest),
            None => (None, pattern),
        };
        if server.contains('/') {
            let net = IpNet::parse(server)
                .map_err(|e| format!("invalid bypass CIDR {pattern:?}: {e}"))?;
            return Ok(Self::Entry {
                scheme,
                host: BypassHost::Cidr(net),
                port: None,
            });
        }
        let mut port = None;
        let mut ip = parse_ip_literal(server);
        if ip.is_none() {
            if let Some(pos) = server.rfind(':') {
                let p = &server[pos + 1..];
                port =
                    Some(p.parse::<u16>().map_err(|_| {
                        format!("invalid port {p:?} in bypass pattern {pattern:?}")
                    })?);
                server = &server[..pos];
                ip = parse_ip_literal(server);
            }
        }
        if server.is_empty() {
            return Err(format!("bypass pattern {pattern:?} has no host"));
        }
        let host = match ip {
            Some(ip) => BypassHost::Ip(ip),
            None => {
                let server = if server.starts_with('.') {
                    format!("*{server}")
                } else {
                    server.to_string()
                };
                if server == "*" {
                    BypassHost::Any
                } else {
                    BypassHost::Wildcard(
                        build_regex("bypass", &shexp_to_regex(&server, true), true)
                            .map_err(|e| e.message)?,
                    )
                }
            }
        };
        Ok(Self::Entry { scheme, host, port })
    }

    /// Whether this entry matches the request (never resolves DNS).
    pub fn matches(&self, ctx: &MatchContext<'_>) -> bool {
        match self {
            Self::Local => {
                matches!(ctx.ip, Some(ip) if ip == IpAddr::V4(Ipv4Addr::LOCALHOST)
                    || ip == IpAddr::V6(Ipv6Addr::LOCALHOST))
                    || !ctx.host.contains('.')
            }
            Self::Entry { scheme, host, port } => {
                if let Some(s) = scheme {
                    if ctx.scheme.map(str::to_ascii_lowercase).as_deref() != Some(s.as_str()) {
                        return false;
                    }
                }
                if let Some(p) = port {
                    if ctx.port != *p {
                        return false;
                    }
                }
                match host {
                    BypassHost::Any => true,
                    BypassHost::Ip(ip) => ctx.ip == Some(*ip),
                    BypassHost::Cidr(net) => ctx.ip.is_some_and(|ip| net.contains(ip)),
                    BypassHost::Wildcard(re) => re.is_match(ctx.host),
                }
            }
        }
    }
}

/// A compiled bypass list (rule `bypass` field or profile-level `bypass`).
#[derive(Debug, Clone, Default)]
pub struct BypassList(Vec<BypassPattern>);

impl BypassList {
    pub fn compile(items: &[String]) -> Result<Self, ConditionError> {
        items
            .iter()
            .map(|s| BypassPattern::parse(s).map_err(|m| ConditionError::new("bypass", m)))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether any entry matches.
    pub fn matches(&self, ctx: &MatchContext<'_>) -> bool {
        self.0.iter().any(|p| p.matches(ctx))
    }
}

// ---------------------------------------------------------------------------
// Time / weekday parsing
// ---------------------------------------------------------------------------

fn parse_weekday(s: &str) -> Option<u8> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "mon" | "monday" => 0,
        "tue" | "tues" | "tuesday" => 1,
        "wed" | "wednesday" => 2,
        "thu" | "thur" | "thurs" | "thursday" => 3,
        "fri" | "friday" => 4,
        "sat" | "saturday" => 5,
        "sun" | "sunday" => 6,
        _ => return None,
    })
}

/// Parse `mon`, `mon-fri`, or a wrapping range like `fri-mon` into a 7-day mask.
fn parse_weekday_spec(spec: &str, days: &mut [bool; 7]) -> Result<(), ConditionError> {
    let err = || {
        ConditionError::new(
            "weekday",
            format!("invalid weekday {spec:?} (use mon..sun or a range like mon-fri)"),
        )
    };
    match spec.split_once('-') {
        Some((a, b)) => {
            let (a, b) = (
                parse_weekday(a).ok_or_else(err)?,
                parse_weekday(b).ok_or_else(err)?,
            );
            let mut d = a;
            loop {
                days[usize::from(d)] = true;
                if d == b {
                    break;
                }
                d = (d + 1) % 7;
            }
        }
        None => days[usize::from(parse_weekday(spec).ok_or_else(err)?)] = true,
    }
    Ok(())
}

/// Parse `HH:MM` (or bare `HH`, meaning `HH:00` as a start and `HH:59` as an end).
fn parse_clock(s: &str, is_end: bool) -> Option<u16> {
    let s = s.trim();
    let (h, m) = match s.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h.parse::<u16>().ok()?, m.parse::<u16>().ok()?),
        Some(_) => return None,
        None => (s.parse::<u16>().ok()?, if is_end { 59 } else { 0 }),
    };
    (h < 24 && m < 60 && !s.is_empty()).then_some(h * 60 + m)
}

/// Parse a `"09:00-17:30"` window into inclusive `(start, end)` minutes of the day.
fn parse_time_window(spec: &str) -> Result<(u16, u16), ConditionError> {
    let err = || {
        ConditionError::new(
            "time",
            format!("invalid time window {spec:?} (use \"HH:MM-HH:MM\", e.g. \"09:00-17:30\")"),
        )
    };
    let (a, b) = spec.split_once('-').ok_or_else(err)?;
    Ok((
        parse_clock(a, false).ok_or_else(err)?,
        parse_clock(b, true).ok_or_else(err)?,
    ))
}

fn parse_port_spec(spec: &PortSpec) -> Result<(u16, u16), ConditionError> {
    match spec {
        PortSpec::Port(p) => Ok((*p, *p)),
        PortSpec::Range(s) => {
            let err = || {
                ConditionError::new(
                    "port",
                    format!("invalid port or range {s:?} (use 443 or \"8000-8080\")"),
                )
            };
            let (a, b) = match s.split_once('-') {
                Some((a, b)) => (a.trim(), b.trim()),
                None => (s.trim(), s.trim()),
            };
            let (a, b) = (
                a.parse::<u16>().map_err(|_| err())?,
                b.parse::<u16>().map_err(|_| err())?,
            );
            if a > b {
                return Err(err());
            }
            Ok((a, b))
        }
    }
}

// ---------------------------------------------------------------------------
// Conditions
// ---------------------------------------------------------------------------

/// One compiled condition field.
#[derive(Debug, Clone)]
pub enum Condition {
    Inbound(Inbound),
    /// Exact normalized hostnames.
    Host(NameSet),
    /// Boxed so the common conditions stay small (rules are scanned in bulk).
    HostWildcard(Box<HostWildcardMatcher>),
    HostRegex(Vec<Regex>),
    /// `cidr` / `ip` networks; hostnames are resolved only when `resolve_dns`.
    Cidr {
        nets: Vec<IpNet>,
        resolve_dns: bool,
    },
    /// Inclusive port ranges.
    Port(Vec<(u16, u16)>),
    /// Inclusive dot-count range (AirSwitch `HostLevelsCondition`).
    HostLevels {
        min: u32,
        max: u32,
    },
    Bypass(BypassList),
    /// Days of week, `0` = Monday.
    Weekday([bool; 7]),
    /// Inclusive minute-of-day windows; `start > end` wraps around midnight.
    Time(Vec<(u16, u16)>),
    UrlWildcard(Regex),
    UrlRegex(Vec<Regex>),
    Keyword(Vec<String>),
}

impl Condition {
    /// Whether this condition only evaluates against a visible full URL.
    pub fn is_url_condition(&self) -> bool {
        matches!(
            self,
            Self::UrlWildcard(_) | Self::UrlRegex(_) | Self::Keyword(_)
        )
    }

    /// Evaluate against one request.
    pub fn matches(&self, ctx: &MatchContext<'_>) -> bool {
        match self {
            Self::Inbound(i) => ctx.inbound == *i,
            Self::Host(hosts) => hosts.contains(ctx.host),
            Self::HostWildcard(m) => m.is_match(ctx.host),
            Self::HostRegex(res) => res.iter().any(|re| re.is_match(ctx.host)),
            Self::Cidr { nets, resolve_dns } => match ctx.ip {
                Some(ip) => nets.iter().any(|n| n.contains(ip)),
                None if *resolve_dns => ctx
                    .resolved()
                    .iter()
                    .any(|ip| nets.iter().any(|n| n.contains(*ip))),
                None => false,
            },
            Self::Port(ranges) => ranges.iter().any(|(a, b)| (*a..=*b).contains(&ctx.port)),
            Self::HostLevels { min, max } => {
                let dots = ctx.host.bytes().filter(|b| *b == b'.').count();
                let dots = u32::try_from(dots).unwrap_or(u32::MAX);
                (*min..=*max).contains(&dots)
            }
            Self::Bypass(list) => list.matches(ctx),
            Self::Weekday(days) => days[usize::from(ctx.now().weekday % 7)],
            Self::Time(windows) => {
                let m = ctx.now().minute_of_day;
                windows.iter().any(|(a, b)| {
                    if a <= b {
                        (*a..=*b).contains(&m)
                    } else {
                        m >= *a || m <= *b
                    }
                })
            }
            Self::UrlWildcard(re) => ctx.url.is_some_and(|u| re.is_match(u)),
            Self::UrlRegex(res) => ctx.url.is_some_and(|u| res.iter().any(|re| re.is_match(u))),
            Self::Keyword(words) => ctx
                .url
                .is_some_and(|u| words.iter().any(|w| u.contains(w.as_str()))),
        }
    }
}

/// A compiled `[[routing.rules]]` entry's matching logic.
#[derive(Debug, Clone)]
pub struct RuleMatcher {
    pub conditions: Vec<Condition>,
    pub match_all: bool,
    pub disabled: bool,
}

/// The host keys of a rule whose only condition is `host` or `host_wildcard` (see
/// [`RuleMatcher::host_only`]), for the router's first-match index.
pub(crate) enum HostOnly<'a> {
    /// A disabled rule: never matches.
    Never,
    Keys {
        /// Exact names (`host` values and plain `host_wildcard` patterns).
        exact: Vec<&'a str>,
        /// `*.d`: `d` and its subdomains.
        apex_and_subdomains: Vec<&'a str>,
        /// `**.d`: subdomains of `d` only.
        subdomains: Vec<&'a str>,
        /// A `*` pattern: matches every host.
        any: bool,
        /// Some patterns are only answered by the rule's own matcher.
        residual: bool,
    },
}

impl RuleMatcher {
    /// Classify this rule for the first-match index: `None` when it has any condition other
    /// than a single `host` or `host_wildcard` (or is `match_all`), so it must be evaluated
    /// on its own. The keys are lowercase ASCII; the index answers ASCII hosts only.
    pub(crate) fn host_only(&self) -> Option<HostOnly<'_>> {
        if self.disabled {
            return Some(HostOnly::Never);
        }
        if self.match_all {
            return None;
        }
        match self.conditions.as_slice() {
            [Condition::Host(names)] => Some(HostOnly::Keys {
                exact: names.iter().collect(),
                apex_and_subdomains: Vec::new(),
                subdomains: Vec::new(),
                any: false,
                residual: false,
            }),
            [Condition::HostWildcard(m)] => Some(HostOnly::Keys {
                exact: m.exact.iter().collect(),
                apex_and_subdomains: m.apex_and_subdomains.iter().collect(),
                subdomains: m.subdomains.iter().collect(),
                any: m.any,
                residual: m.residual.is_some(),
            }),
            _ => None,
        }
    }
}

impl RuleMatcher {
    /// Compile a rule. `global_resolve_dns` is `[routing].resolve_dns`; the rule's own
    /// `resolve_dns` overrides it.
    pub fn compile(rule: &RuleConfig, global_resolve_dns: bool) -> Result<Self, ConditionError> {
        let mut c = Vec::new();
        if let Some(i) = rule.inbound {
            c.push(Condition::Inbound(i));
        }
        if let Some(v) = &rule.host {
            let hosts = values("host", v)?
                .iter()
                .map(|h| normalize_host(h))
                .collect::<Vec<_>>();
            if hosts.iter().any(String::is_empty) {
                return Err(ConditionError::new("host", "contains an empty hostname"));
            }
            c.push(Condition::Host(NameSet::from_names(hosts)));
        }
        if let Some(v) = &rule.host_wildcard {
            let pats = split_patterns("host_wildcard", values("host_wildcard", v)?)?;
            c.push(Condition::HostWildcard(Box::new(
                HostWildcardMatcher::compile(&pats)?,
            )));
        }
        if let Some(v) = &rule.host_regex {
            let res = regex_list("host_regex", values("host_regex", v)?, false)?;
            c.push(Condition::HostRegex(res));
        }
        let mut nets = Vec::new();
        for (field, v) in [("cidr", &rule.cidr), ("ip", &rule.ip)] {
            if let Some(v) = v {
                for s in values(field, v)? {
                    nets.push(IpNet::parse(s).map_err(|m| ConditionError::new(field, m))?);
                }
            }
        }
        if !nets.is_empty() {
            c.push(Condition::Cidr {
                nets,
                resolve_dns: rule.resolve_dns.unwrap_or(global_resolve_dns),
            });
        }
        if let Some(v) = &rule.port {
            let ranges = values("port", v)?
                .iter()
                .map(parse_port_spec)
                .collect::<Result<Vec<_>, _>>()?;
            c.push(Condition::Port(ranges));
        }
        if let Some(HostLevels { min, max }) = rule.host_levels {
            let (min, max) = (min.unwrap_or(0), max.unwrap_or(u32::MAX));
            if min > max {
                return Err(ConditionError::new(
                    "host_levels",
                    format!("min ({min}) is greater than max ({max})"),
                ));
            }
            c.push(Condition::HostLevels { min, max });
        }
        if let Some(v) = &rule.bypass {
            c.push(Condition::Bypass(BypassList::compile(values(
                "bypass", v,
            )?)?));
        }
        if let Some(v) = &rule.weekday {
            let mut days = [false; 7];
            for spec in values("weekday", v)? {
                for part in spec.split(',').filter(|p| !p.trim().is_empty()) {
                    parse_weekday_spec(part.trim(), &mut days)?;
                }
            }
            if !days.iter().any(|d| *d) {
                return Err(ConditionError::new("weekday", "lists no day"));
            }
            c.push(Condition::Weekday(days));
        }
        if let Some(v) = &rule.time {
            let windows = values("time", v)?
                .iter()
                .map(|s| parse_time_window(s))
                .collect::<Result<Vec<_>, _>>()?;
            c.push(Condition::Time(windows));
        }
        if let Some(v) = &rule.url_wildcard {
            let pats = split_patterns("url_wildcard", values("url_wildcard", v)?)?;
            let sources: Vec<String> = pats.iter().map(|p| shexp_to_regex(p, true)).collect();
            c.push(Condition::UrlWildcard(alternation(
                "url_wildcard",
                &sources,
                false,
            )?));
        }
        if let Some(v) = &rule.url_regex {
            let res = regex_list("url_regex", values("url_regex", v)?, false)?;
            c.push(Condition::UrlRegex(res));
        }
        if let Some(v) = &rule.keyword {
            let words = values("keyword", v)?.to_vec();
            if words.iter().any(String::is_empty) {
                return Err(ConditionError::new("keyword", "contains an empty keyword"));
            }
            c.push(Condition::Keyword(words));
        }
        if c.is_empty() && !rule.match_all && !rule.disabled {
            return Err(ConditionError::new(
                "rule",
                "has no conditions; add a matcher or set `match_all = true`",
            ));
        }
        Ok(Self {
            conditions: c,
            match_all: rule.match_all,
            disabled: rule.disabled,
        })
    }

    /// Whether the rule uses URL-only conditions (`url_wildcard`, `url_regex`, `keyword`).
    pub fn has_url_conditions(&self) -> bool {
        !self.disabled && self.conditions.iter().any(Condition::is_url_condition)
    }

    /// Whether the rule has a `cidr`/`ip` condition with `resolve_dns` on, so routing a
    /// hostname may need a DNS lookup. Disabled rules never need one.
    pub fn needs_dns(&self) -> bool {
        !self.disabled
            && self.conditions.iter().any(|c| {
                matches!(
                    c,
                    Condition::Cidr {
                        resolve_dns: true,
                        ..
                    }
                )
            })
    }

    /// AND across all conditions; disabled rules never match.
    pub fn matches(&self, ctx: &MatchContext<'_>) -> bool {
        !self.disabled && self.conditions.iter().all(|c| c.matches(ctx))
    }

    /// Whether the rule could match some request to `host:port`, judging only its host,
    /// IP/CIDR, port, and host-level conditions. `host` must be normalized (see
    /// [`normalize_host`]) and `ip` is its IP literal, if it is one. Conservative: any
    /// other condition, and a `cidr`/`ip` condition with `resolve_dns` on a hostname, never
    /// rules the host out. Never resolves DNS. Disabled rules never match.
    pub fn may_match_host_port(&self, host: &str, ip: Option<IpAddr>, port: u16) -> bool {
        !self.disabled
            && self.conditions.iter().all(|c| match c {
                Condition::Host(hosts) => hosts.contains(host),
                Condition::HostWildcard(m) => m.is_match(host),
                Condition::HostRegex(res) => res.iter().any(|re| re.is_match(host)),
                Condition::Cidr { nets, resolve_dns } => match ip {
                    Some(ip) => nets.iter().any(|n| n.contains(ip)),
                    None => *resolve_dns,
                },
                Condition::Port(ranges) => ranges.iter().any(|(a, b)| (*a..=*b).contains(&port)),
                Condition::HostLevels { min, max } => {
                    let dots = host.bytes().filter(|b| *b == b'.').count();
                    let dots = u32::try_from(dots).unwrap_or(u32::MAX);
                    (*min..=*max).contains(&dots)
                }
                Condition::Inbound(_)
                | Condition::Bypass(_)
                | Condition::Weekday(_)
                | Condition::Time(_)
                | Condition::UrlWildcard(_)
                | Condition::UrlRegex(_)
                | Condition::Keyword(_) => true,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_queries_the_local_time_once_per_minute() {
        // Another test may run in the same minute; count the delta of two back-to-back calls.
        let first = SystemClock.now();
        let before = LOCAL_TIME_QUERIES.load(Ordering::Relaxed);
        let second = SystemClock.now();
        let after = LOCAL_TIME_QUERIES.load(Ordering::Relaxed);
        if first.minute_of_day == second.minute_of_day {
            assert_eq!(
                after, before,
                "a second call in the same minute must use the cache"
            );
            assert_eq!(first, second);
        }
        let real = LocalTime::now();
        assert!(second.weekday < 7 && second.minute_of_day < 1440);
        // Same minute unless the clock ticked over between the calls.
        assert!(real == second || real.minute_of_day != second.minute_of_day);
    }

    #[test]
    fn normalize_host_cow_matches_normalize_host() {
        for host in [
            "example.com",
            "Example.COM.",
            " [::1] ",
            "[FE80::1]",
            "a.",
            "",
            "é.Lan",
            "x",
        ] {
            assert_eq!(normalize_host_cow(host), normalize_host(host), "{host:?}");
        }
        assert!(matches!(
            normalize_host_cow("example.com"),
            Cow::Borrowed(_)
        ));
    }
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingResolver {
        calls: AtomicUsize,
        answer: Vec<IpAddr>,
    }

    impl CountingResolver {
        fn new(answer: &[&str]) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                answer: answer.iter().map(|s| s.parse().unwrap()).collect(),
            }
        }
    }

    impl Resolver for CountingResolver {
        fn resolve(&self, _host: &str, _port: u16) -> Vec<IpAddr> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
    }

    const MON_NOON: FixedClock = FixedClock(LocalTime {
        weekday: 0,
        minute_of_day: 12 * 60,
    });

    fn rule(toml_body: &str) -> RuleConfig {
        toml::from_str(&format!("target = \"direct\"\n{toml_body}")).unwrap()
    }

    fn matcher(toml_body: &str) -> RuleMatcher {
        RuleMatcher::compile(&rule(toml_body), false).unwrap()
    }

    fn compile_err(toml_body: &str) -> ConditionError {
        RuleMatcher::compile(&rule(toml_body), false).unwrap_err()
    }

    struct Req<'a> {
        inbound: Inbound,
        host: &'a str,
        port: u16,
        scheme: Option<&'a str>,
        url: Option<&'a str>,
        clock: &'a dyn Clock,
        resolver: &'a dyn Resolver,
    }

    impl<'a> Req<'a> {
        fn host(host: &'a str) -> Self {
            Self {
                inbound: Inbound::Http,
                host,
                port: 443,
                scheme: None,
                url: None,
                clock: &MON_NOON,
                resolver: &SystemResolver,
            }
        }
        fn matches(&self, m: &RuleMatcher) -> bool {
            let host = normalize_host(self.host);
            let ctx = MatchContext::new(
                self.inbound,
                &host,
                self.port,
                self.scheme,
                self.url,
                self.clock,
                self.resolver,
            );
            m.matches(&ctx)
        }
    }

    fn host_matches(m: &RuleMatcher, host: &str) -> bool {
        Req::host(host).matches(m)
    }

    #[test]
    fn inbound_condition() {
        let m = matcher("inbound = \"socks\"\nmatch_all = true");
        assert!(!Req::host("a.example").matches(&m));
        let mut r = Req::host("a.example");
        r.inbound = Inbound::Socks;
        assert!(r.matches(&m));
    }

    #[test]
    fn host_exact_is_case_insensitive_and_strips_trailing_dot() {
        let m = matcher("host = [\"Example.COM.\", \"b.example\"]");
        assert!(host_matches(&m, "example.com"));
        assert!(host_matches(&m, "EXAMPLE.com."));
        assert!(host_matches(&m, "b.example"));
        assert!(!host_matches(&m, "www.example.com"));
        assert!(compile_err("host = []").message.contains("at least one"));
    }

    /// Small deterministic generator for the equivalence tests (no `rand` dependency).
    #[cfg(feature = "routing")]
    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }

    #[cfg(feature = "routing")]
    #[test]
    fn host_wildcard_fast_paths_agree_with_the_full_regex() {
        let patterns = [
            "*.example.com",
            "**.example.com",
            ".lan",
            "example.org",
            "*",
            "dev-*.corp.net",
            "h?st.local",
            "*.example.*",
            "a+b.example",
            "*.Mixed.Case",
            "**.deep.a.b",
            "exact.only",
            "*.",
            "**",
            "*.é.example",
            "x.*.example",
            "*.a..b",
        ];
        let mut hosts: Vec<String> = [
            "example.com",
            "www.example.com",
            "a.b.example.com",
            "badexample.com",
            "example.com.evil",
            "lan",
            "nas.lan",
            "example.org",
            "x.example.org",
            "dev-1.corp.net",
            "host.local",
            "hoost.local",
            "example.anything",
            "a+b.example",
            "mixed.case",
            "q.mixed.case",
            "deep.a.b",
            "z.deep.a.b",
            "exact.only",
            "",
            "\u{212a}.example.com",
            "café.example",
            "x.é.example",
            "x.y.example",
            "q.a..b",
            "WWW.EXAMPLE.COM",
        ]
        .iter()
        .map(|h| h.to_string())
        .collect();
        let mut state = 7u64;
        let labels = [
            "example", "com", "lan", "a", "b", "deep", "corp", "net", "x", "dev-1",
        ];
        for _ in 0..2000 {
            let n = 1 + lcg(&mut state) % 4;
            let host: Vec<&str> = (0..n)
                .map(|_| labels[(lcg(&mut state) % labels.len() as u64) as usize])
                .collect();
            hosts.push(host.join("."));
        }
        // Every pattern alone, and all of them in one field.
        let mut fields: Vec<Vec<String>> = patterns.iter().map(|p| vec![p.to_string()]).collect();
        fields.push(patterns.iter().map(|p| p.to_string()).collect());
        for field in fields {
            let m = HostWildcardMatcher::compile(&field).unwrap();
            for host in &hosts {
                assert_eq!(
                    m.is_match(host),
                    m.full_is_match(host),
                    "patterns {field:?} host {host:?}"
                );
            }
        }
    }

    #[cfg(feature = "routing")]
    #[test]
    fn combined_regex_lists_agree_with_separate_regexes() {
        let sources: Vec<String> = ["^r1\\.", "(?i)^MIXED$", "corp$", "^a|b$", "x{2,}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let combined = regex_list("host_regex", &sources, false).unwrap();
        assert_eq!(combined.len(), 1, "patterns combine into one regex");
        let separate: Vec<Regex> = sources
            .iter()
            .map(|s| build_regex("host_regex", s, false).unwrap())
            .collect();
        for host in [
            "r1.x", "r10.x", "mixed", "MIXED", "my.corp", "corp.x", "a", "zb", "axx", "q",
        ] {
            assert_eq!(
                combined.iter().any(|re| re.is_match(host)),
                separate.iter().any(|re| re.is_match(host)),
                "{host}"
            );
        }
        let err =
            regex_list("host_regex", &["ok".to_string(), "(".to_string()], false).unwrap_err();
        assert!(err.message.contains("\"(\""), "{}", err.message);
    }

    #[cfg(feature = "routing")]
    #[test]
    fn host_wildcard_apex_and_subdomains() {
        let m = matcher("host_wildcard = \"*.example.com\"");
        assert!(host_matches(&m, "example.com"));
        assert!(host_matches(&m, "www.example.com"));
        assert!(host_matches(&m, "a.b.EXAMPLE.com"));
        assert!(!host_matches(&m, "badexample.com"));
        assert!(!host_matches(&m, "example.com.evil"));
    }

    #[cfg(feature = "routing")]
    #[test]
    fn host_wildcard_subdomains_only() {
        let m = matcher("host_wildcard = \"**.example.com\"");
        assert!(!host_matches(&m, "example.com"));
        assert!(host_matches(&m, "www.example.com"));
        assert!(host_matches(&m, "a.b.example.com"));
        assert!(!host_matches(&m, "badexample.com"));
    }

    #[cfg(feature = "routing")]
    #[test]
    fn host_wildcard_star_question_and_pipes() {
        let m = matcher("host_wildcard = [\"dev-*.corp.net\", \"h?st.local|.lan\"]");
        assert!(host_matches(&m, "dev-1.corp.net"));
        assert!(host_matches(&m, "dev-abc.corp.net"));
        assert!(!host_matches(&m, "prod.corp.net"));
        assert!(host_matches(&m, "host.local"));
        assert!(!host_matches(&m, "hoost.local"));
        // `.lan` is shorthand for `*.lan` (apex + subdomains).
        assert!(host_matches(&m, "lan"));
        assert!(host_matches(&m, "nas.lan"));
        let any = matcher("host_wildcard = \"*\"");
        assert!(host_matches(&any, "anything.at.all"));
        assert!(host_matches(&any, "10.0.0.1"));
        // Literal regex metacharacters are escaped.
        let lit = matcher("host_wildcard = \"a+b.example\"");
        assert!(host_matches(&lit, "a+b.example"));
        assert!(!host_matches(&lit, "aab.example"));
        assert!(compile_err("host_wildcard = \"|\"").field == "host_wildcard");
    }

    #[cfg(feature = "routing")]
    #[test]
    fn host_regex_condition() {
        let m = matcher("host_regex = \"^telemetry\\\\..*\\\\.example\\\\.com$\"");
        assert!(host_matches(&m, "telemetry.eu.example.com"));
        assert!(!host_matches(&m, "www.example.com"));
        let e = compile_err("host_regex = \"(unclosed\"");
        assert_eq!(e.field, "host_regex");
    }

    #[test]
    fn cidr_matches_literals_and_canonicalizes_mapped_ipv6() {
        let m = matcher("cidr = [\"10.0.0.0/8\", \"fd00::/8\"]\nip = \"192.168.1.1\"");
        assert!(host_matches(&m, "10.1.2.3"));
        assert!(host_matches(&m, "::ffff:10.9.9.9"));
        assert!(host_matches(&m, "[::ffff:10.9.9.9]"));
        assert!(host_matches(&m, "fd12::1"));
        assert!(host_matches(&m, "192.168.1.1"));
        assert!(!host_matches(&m, "192.168.1.2"));
        assert!(!host_matches(&m, "11.0.0.1"));
        // A mapped-IPv6 CIDR is folded to its IPv4 equivalent.
        let mapped = matcher("cidr = \"::ffff:172.16.0.0/108\"");
        assert!(host_matches(&mapped, "172.16.5.5"));
        assert!(!host_matches(&mapped, "172.32.0.1"));
        assert_eq!(compile_err("cidr = \"10.0.0.0/33\"").field, "cidr");
        assert_eq!(compile_err("ip = \"not-an-ip\"").field, "ip");
    }

    #[test]
    fn cidr_never_resolves_hostnames_by_default() {
        let resolver = CountingResolver::new(&["10.0.0.5"]);
        let m = matcher("cidr = \"10.0.0.0/8\"");
        let mut r = Req::host("intranet.example");
        r.resolver = &resolver;
        assert!(!r.matches(&m));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cidr_resolves_hostnames_only_when_opted_in() {
        let resolver = CountingResolver::new(&["10.0.0.5"]);
        let per_rule = matcher("cidr = \"10.0.0.0/8\"\nresolve_dns = true");
        let mut r = Req::host("intranet.example");
        r.resolver = &resolver;
        assert!(r.matches(&per_rule));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        // Global opt-in, with per-rule opt-out winning.
        let global = RuleMatcher::compile(&rule("cidr = \"10.0.0.0/8\""), true).unwrap();
        assert!(r.matches(&global));
        let opt_out =
            RuleMatcher::compile(&rule("cidr = \"10.0.0.0/8\"\nresolve_dns = false"), true)
                .unwrap();
        let before = resolver.calls.load(Ordering::SeqCst);
        assert!(!r.matches(&opt_out));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), before);
        // IP literals never hit the resolver even with resolve_dns.
        let mut lit = Req::host("10.1.1.1");
        lit.resolver = &resolver;
        assert!(lit.matches(&per_rule));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), before);
    }

    #[test]
    fn port_single_list_and_range() {
        let m = matcher("port = [22, \"2200-2299\", \"8080\"]");
        let at = |p: u16| {
            let mut r = Req::host("h");
            r.port = p;
            r.matches(&m)
        };
        assert!(at(22));
        assert!(at(2200) && at(2250) && at(2299));
        assert!(at(8080));
        assert!(!at(23) && !at(2300) && !at(443));
        let one = matcher("port = 443");
        assert!(Req::host("h").matches(&one));
        assert_eq!(compile_err("port = \"90-80\"").field, "port");
        assert_eq!(compile_err("port = \"abc\"").field, "port");
    }

    #[test]
    fn host_levels_counts_dots() {
        let m = matcher("host_levels = { min = 1, max = 2 }");
        assert!(!host_matches(&m, "localhost"));
        assert!(host_matches(&m, "example.com"));
        assert!(host_matches(&m, "www.example.com"));
        assert!(!host_matches(&m, "a.www.example.com"));
        let min_only = matcher("host_levels = { min = 3 }");
        assert!(host_matches(&min_only, "a.b.c.d.e"));
        assert_eq!(
            compile_err("host_levels = { min = 3, max = 1 }").field,
            "host_levels"
        );
    }

    #[cfg(feature = "routing")]
    #[test]
    fn bypass_local_wildcard_cidr_and_host_port() {
        let m = matcher("bypass = \"<local>\"");
        assert!(host_matches(&m, "127.0.0.1"));
        assert!(host_matches(&m, "::1"));
        assert!(host_matches(&m, "intranet"));
        assert!(!host_matches(&m, "127.0.0.2"));
        assert!(!host_matches(&m, "www.example.com"));

        // Bypass `*.domain` is a shell wildcard: subdomains only.
        let w = matcher("bypass = [\"*.local\", \"exact.example\"]");
        assert!(host_matches(&w, "printer.local"));
        assert!(!host_matches(&w, "local"));
        assert!(host_matches(&w, "exact.example"));
        assert!(!host_matches(&w, "www.exact.example"));

        let cidr = matcher("bypass = [\"10.0.0.0/8\", \"[::1]\", \"192.168.0.1\"]");
        assert!(host_matches(&cidr, "10.2.3.4"));
        assert!(!host_matches(&cidr, "ten.example"));
        assert!(host_matches(&cidr, "::1"));
        assert!(host_matches(&cidr, "192.168.0.1"));

        let hp =
            matcher("bypass = [\"db.example:5432\", \"[fd00::1]:22\", \"https://secure.example\"]");
        let mut r = Req::host("db.example");
        r.port = 5432;
        assert!(r.matches(&hp));
        r.port = 5433;
        assert!(!r.matches(&hp));
        let mut v6 = Req::host("fd00::1");
        v6.port = 22;
        assert!(v6.matches(&hp));
        let mut s = Req::host("secure.example");
        assert!(!s.matches(&hp));
        s.scheme = Some("https");
        assert!(s.matches(&hp));

        assert_eq!(compile_err("bypass = \"host:notaport\"").field, "bypass");
        assert_eq!(compile_err("bypass = \"10.0.0.0/99\"").field, "bypass");
    }

    #[test]
    fn weekday_lists_ranges_and_wrap() {
        let at = |m: &RuleMatcher, day: u8| {
            let clock = FixedClock(LocalTime::new(day, 12, 0));
            let mut r = Req::host("h");
            r.clock = &clock;
            r.matches(m)
        };
        let work = matcher("weekday = \"mon-fri\"");
        assert!((0..5).all(|d| at(&work, d)));
        assert!(!at(&work, 5) && !at(&work, 6));
        let list = matcher("weekday = [\"sat\", \"Sunday\"]");
        assert!(at(&list, 5) && at(&list, 6) && !at(&list, 0));
        let wrap = matcher("weekday = \"fri-mon\"");
        assert!(at(&wrap, 4) && at(&wrap, 5) && at(&wrap, 6) && at(&wrap, 0));
        assert!(!at(&wrap, 1) && !at(&wrap, 3));
        assert_eq!(compile_err("weekday = \"funday\"").field, "weekday");
    }

    #[test]
    fn time_windows_with_midnight_wrap() {
        let at = |m: &RuleMatcher, h: u8, min: u8| {
            let clock = FixedClock(LocalTime::new(0, h, min));
            let mut r = Req::host("h");
            r.clock = &clock;
            r.matches(m)
        };
        let office = matcher("time = \"09:00-17:30\"");
        assert!(at(&office, 9, 0) && at(&office, 17, 30) && at(&office, 12, 0));
        assert!(!at(&office, 8, 59) && !at(&office, 17, 31));
        let night = matcher("time = \"22:00-06:00\"");
        assert!(at(&night, 23, 0) && at(&night, 0, 0) && at(&night, 6, 0));
        assert!(!at(&night, 6, 1) && !at(&night, 21, 59));
        let hours = matcher("time = [\"08-08\"]");
        assert!(at(&hours, 8, 0) && at(&hours, 8, 59) && !at(&hours, 9, 0));
        assert_eq!(compile_err("time = \"25:00-26:00\"").field, "time");
        assert_eq!(compile_err("time = \"9am\"").field, "time");
    }

    #[cfg(feature = "routing")]
    #[test]
    fn url_conditions_need_a_visible_url() {
        let w = matcher("url_wildcard = \"http://downloads.example.com/private/*\"");
        let re = matcher("url_regex = \"^https://api\\\\.example\\\\.com/v[0-9]+/\"");
        let kw = matcher("keyword = [\"tracking\"]");
        let with_url = |m: &RuleMatcher, url: Option<&str>| {
            let mut r = Req::host("downloads.example.com");
            r.url = url;
            r.matches(m)
        };
        assert!(with_url(
            &w,
            Some("http://downloads.example.com/private/a.zip")
        ));
        assert!(!with_url(
            &w,
            Some("http://downloads.example.com/public/a.zip")
        ));
        assert!(!with_url(&w, None));
        assert!(with_url(&re, Some("https://api.example.com/v2/users")));
        assert!(!with_url(&re, Some("https://api.example.com/users")));
        assert!(!with_url(&re, None));
        assert!(with_url(&kw, Some("http://x.example/?tracking=1")));
        assert!(!with_url(&kw, Some("http://x.example/")));
        assert!(!with_url(&kw, None));
        assert!(w.has_url_conditions() && re.has_url_conditions() && kw.has_url_conditions());
        assert_eq!(compile_err("url_regex = \"[\"").field, "url_regex");
    }

    #[cfg(feature = "routing")]
    #[test]
    fn match_all_disabled_and_and_semantics() {
        let all = matcher("match_all = true");
        assert!(host_matches(&all, "anything"));
        assert!(!all.has_url_conditions());
        let disabled = matcher("match_all = true\ndisabled = true");
        assert!(!host_matches(&disabled, "anything"));
        let disabled_url = matcher("keyword = \"x\"\ndisabled = true");
        assert!(!disabled_url.has_url_conditions());
        // AND across fields.
        let and = matcher("host_wildcard = \"*.example.com\"\nport = 22");
        assert!(!host_matches(&and, "a.example.com")); // port 443
        let mut r = Req::host("a.example.com");
        r.port = 22;
        assert!(r.matches(&and));
        assert_eq!(compile_err("name = \"empty\"").field, "rule");
    }

    #[test]
    fn shexp_matches_airswitch_translation() {
        assert_eq!(shexp_to_regex("*.example.com", true), "\\.example\\.com$");
        assert_eq!(shexp_to_regex("a?c*", true), "^a.c");
        assert_eq!(shexp_to_regex("abc", false), "^abc$");
        assert_eq!(
            host_wildcard_regex("*.example.com"),
            "(?:^|\\.)example\\.com$"
        );
        assert_eq!(host_wildcard_regex("**.example.com"), "\\.example\\.com$");
        assert_eq!(host_wildcard_regex("*.example.*"), "(?:^|\\.)example\\.");
    }

    #[cfg(not(feature = "routing"))]
    #[test]
    fn regex_conditions_require_routing_feature() {
        for body in [
            "host_wildcard = \"*.example.com\"",
            "host_regex = \"^a$\"",
            "url_wildcard = \"*/x\"",
            "url_regex = \"^http://\"",
            "bypass = \"*.example.com\"",
        ] {
            let err = compile_err(body);
            assert!(
                err.message.contains("`routing` cargo feature"),
                "{body}: {err}"
            );
        }
        // Non-regex conditions still compile and match.
        let m = matcher("host = \"example.com\"\nport = 443");
        assert!(Req::host("example.com").matches(&m));
        assert!(!Req::host("other.example.com").matches(&m));
        assert_eq!(shexp_to_regex("a.b*", true), "^a\\.b");
    }

    #[test]
    fn ipnet_parse_and_display() {
        assert_eq!(
            IpNet::parse("10.1.2.3/8").unwrap().to_string(),
            "10.0.0.0/8"
        );
        assert_eq!(IpNet::parse("::1").unwrap().to_string(), "::1/128");
        assert_eq!(IpNet::parse("0.0.0.0/0").unwrap().to_string(), "0.0.0.0/0");
        assert!(IpNet::parse("0.0.0.0/0")
            .unwrap()
            .contains("8.8.8.8".parse().unwrap()));
        assert!(!IpNet::parse("0.0.0.0/0")
            .unwrap()
            .contains("::2".parse().unwrap()));
    }
}
