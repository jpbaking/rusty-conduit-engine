//! Auto Switch routing engine.
//!
//! A [`Router`] is compiled once from a validated [`RoutingInput`] (at startup and on every
//! hot-reload) and is immutable afterwards. Each connection (or decrypted MITM request)
//! builds a [`RouteRequest`] and calls [`Router::route`], which:
//!
//! 1. applies the global `[filter]` allow/block list (denials return [`AccessDenied`],
//!    logged as `status=denied_by_filter`);
//! 2. selects the rule table: `[routing.http]` / `[routing.socks]` when present for the
//!    inbound protocol, otherwise the unified `[routing]` table;
//! 3. evaluates rules strictly top to bottom (AND across fields, OR within a field), falling
//!    back to the table default;
//! 4. resolves each name of the target (single profile or failover chain) through composite
//!    per-scheme profiles and profile-level `bypass` lists.
//!
//! Split-table fallback order: the split table's rules; then, with `inherit = true`, the
//! unified `[routing]` rules; then the split table's `default` if set, else (`inherit`) the
//! unified default, else `direct`.

pub mod condition;
pub mod validate;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::net::{IpAddr, SocketAddr};

use crate::filter::{AccessDenied, CompiledFilter};
use crate::routing::{
    Inbound, ProfileConfig, ProfileType, RoutingInput, RuleConfig, Target, PROFILE_DIRECT,
};

pub use condition::{
    normalize_host, BypassList, Clock, FixedClock, LocalTime, MatchContext, Resolver, RuleMatcher,
    SystemClock, SystemResolver,
};
pub use validate::RouterError;

/// One routing query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteRequest<'a> {
    pub inbound: Inbound,
    /// Hostname or IP literal (brackets and a trailing dot are tolerated).
    pub host: &'a str,
    pub port: u16,
    /// `Some("http")` / `Some("https")` when the scheme is known; selects the sub-target of
    /// composite profiles. Derived from `url` when absent.
    pub scheme: Option<&'a str>,
    /// Full URL when visible (plain HTTP or decrypted MITM HTTPS); URL rules need it.
    pub url: Option<&'a str>,
}

impl<'a> RouteRequest<'a> {
    /// A request without scheme or URL (SOCKS connection, opaque `CONNECT` tunnel).
    pub fn new(inbound: Inbound, host: &'a str, port: u16) -> Self {
        Self {
            inbound,
            host,
            port,
            scheme: None,
            url: None,
        }
    }
}

/// Which rule table produced a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    Unified,
    Http,
    Socks,
}

impl TableKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unified => "[routing]",
            Self::Http => "[routing.http]",
            Self::Socks => "[routing.socks]",
        }
    }
}

impl fmt::Display for TableKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One element of a resolved failover chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// The profile name as written in the rule / default target.
    pub name: String,
    /// The concrete profile to dial (after composite resolution and bypass).
    pub profile: String,
    pub kind: ProfileType,
    /// The target matched a profile-level `bypass` list and resolves to `direct`.
    pub bypassed: bool,
}

impl ResolvedTarget {
    /// Label for `profile=` log fields: `corp`, `corp(bypass->direct)`, or `split->corp`.
    pub fn label(&self) -> String {
        if self.bypassed {
            format!("{}(bypass->direct)", self.name)
        } else if self.name != self.profile {
            format!("{}->{}", self.name, self.profile)
        } else {
            self.name.clone()
        }
    }
}

/// Label used for the `rule=` field when the table default applied.
pub const DEFAULT_RULE_LABEL: &str = "<default>";

/// The outcome of routing one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub table: TableKind,
    /// Name of the matched rule (`name`, or `<table> rule #N`); `None` = table default.
    pub rule: Option<String>,
    /// 0-based index of the matched rule within `table`.
    pub rule_index: Option<usize>,
    /// Ordered failover chain (never empty).
    pub targets: Vec<ResolvedTarget>,
    /// The table(s) consulted for this inbound protocol contain URL rules, so a MITM tunnel
    /// must re-route each decrypted request.
    pub has_url_rules: bool,
    /// Matched rule requested bypassing HTTPS MITM interception (opaque `CONNECT` tunnel).
    pub bypass_mitm: bool,
}

impl RouteDecision {
    /// `rule=` log value.
    pub fn rule_label(&self) -> &str {
        self.rule.as_deref().unwrap_or(DEFAULT_RULE_LABEL)
    }

    /// `profile=` log value of the first target.
    pub fn profile_label(&self) -> String {
        self.targets
            .first()
            .map(ResolvedTarget::label)
            .unwrap_or_default()
    }

    /// Comma-separated labels of the whole chain.
    pub fn chain_label(&self) -> String {
        self.targets
            .iter()
            .map(ResolvedTarget::label)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A [`Resolver`] answering every lookup from a fixed, pre-resolved address list.
struct SliceResolver<'a>(&'a [IpAddr]);

impl Resolver for SliceResolver<'_> {
    fn resolve(&self, _host: &str, _port: u16) -> Vec<IpAddr> {
        self.0
            .iter()
            .copied()
            .map(condition::canonical_ip)
            .collect()
    }
}

#[derive(Debug, Clone)]
struct CompiledRule {
    label: String,
    /// `false` for a rule without `name` (labelled by position, `<table> rule #N`).
    named: bool,
    /// Hash of the rule as written, to tell whether an unnamed rule's position now holds a
    /// different rule after a reload.
    fingerprint: u64,
    matcher: RuleMatcher,
    target: Vec<String>,
    bypass_mitm: bool,
}

/// Stable within one process: the default `SipHash` keys are fixed.
fn rule_fingerprint(rule: &RuleConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(rule).unwrap_or_default().hash(&mut h);
    h.finish()
}

/// A run of consecutive host-only rules (see `RuleMatcher::host_only`), indexed by host key
/// with the lowest rule index per key so the first match keeps rule order.
#[derive(Debug, Clone, Default)]
struct HostRun {
    /// Rule index range `start..end` in the table.
    start: usize,
    end: usize,
    exact: HashMap<String, usize>,
    apex_and_subdomains: HashMap<String, usize>,
    subdomains: HashMap<String, usize>,
    /// Lowest index of a rule with a `*` pattern.
    any: Option<usize>,
    /// Rules (ascending) whose residual patterns need their own matcher.
    residual: Vec<usize>,
}

impl HostRun {
    fn add(&mut self, index: usize, keys: &condition::HostOnly<'_>) {
        self.end = index + 1;
        let condition::HostOnly::Keys {
            exact,
            apex_and_subdomains,
            subdomains,
            any,
            residual,
        } = keys
        else {
            return;
        };
        let keep = |map: &mut HashMap<String, usize>, key: &str| {
            map.entry(key.to_string()).or_insert(index);
        };
        for k in exact {
            keep(&mut self.exact, k);
        }
        for k in apex_and_subdomains {
            keep(&mut self.apex_and_subdomains, k);
        }
        for k in subdomains {
            keep(&mut self.subdomains, k);
        }
        if *any && self.any.is_none() {
            self.any = Some(index);
        }
        if *residual {
            self.residual.push(index);
        }
    }

    /// Lowest index of a rule in this run that matches, for a lowercase ASCII host.
    fn first_match(&self, rules: &[CompiledRule], ctx: &MatchContext<'_>) -> Option<usize> {
        let host = ctx.host;
        let mut best = self.any;
        let mut consider = |hit: Option<&usize>| {
            if let Some(&i) = hit {
                best = Some(best.map_or(i, |b| b.min(i)));
            }
        };
        consider(self.exact.get(host));
        consider(self.apex_and_subdomains.get(host));
        if !self.apex_and_subdomains.is_empty() || !self.subdomains.is_empty() {
            for (i, b) in host.bytes().enumerate() {
                if b == b'.' {
                    let suffix = &host[i + 1..];
                    consider(self.apex_and_subdomains.get(suffix));
                    consider(self.subdomains.get(suffix));
                }
            }
        }
        for &i in &self.residual {
            if best.is_some_and(|b| b <= i) {
                break;
            }
            if rules[i].matcher.matches(ctx) {
                return Some(i);
            }
        }
        best
    }
}

/// A table split into indexed host-only runs and rules evaluated one by one.
#[derive(Debug, Clone)]
enum Segment {
    Run(HostRun),
    /// Rules `start..end`, evaluated one by one.
    Linear(usize, usize),
}

/// Host-only runs shorter than this are scanned (the index costs more than it saves).
const MIN_INDEXED_RUN: usize = 8;

fn segments(rules: &[CompiledRule]) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut run: Option<HostRun> = None;
    // Append `start..end` to a trailing linear segment, or start one.
    fn linear(out: &mut Vec<Segment>, start: usize, end: usize) {
        match out.last_mut() {
            Some(Segment::Linear(_, e)) if *e == start => *e = end,
            _ => out.push(Segment::Linear(start, end)),
        }
    }
    let flush = |run: &mut Option<HostRun>, out: &mut Vec<Segment>| {
        if let Some(r) = run.take() {
            if r.end - r.start >= MIN_INDEXED_RUN {
                out.push(Segment::Run(r));
            } else {
                linear(out, r.start, r.end);
            }
        }
    };
    for (i, rule) in rules.iter().enumerate() {
        match rule.matcher.host_only() {
            Some(keys) => run
                .get_or_insert_with(|| HostRun {
                    start: i,
                    end: i,
                    ..HostRun::default()
                })
                .add(i, &keys),
            None => {
                flush(&mut run, &mut out);
                linear(&mut out, i, i + 1);
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

#[derive(Debug, Clone)]
struct Table {
    kind: TableKind,
    segments: Vec<Segment>,
    rules: Vec<CompiledRule>,
    default: Option<Vec<String>>,
    inherit: bool,
    has_url_rules: bool,
    has_dns_rules: bool,
}

impl Table {
    fn compile(
        cfg: &RoutingInput,
        kind: TableKind,
        rules: &[RuleConfig],
        default: Option<&Target>,
        inherit: bool,
    ) -> Result<Self, RouterError> {
        let table = kind.as_str();
        if let Some(d) = default {
            validate::check_target(&cfg.profiles, &format!("{table}.default"), d)?;
        }
        let matchers =
            validate::compile_rules(&cfg.profiles, table, rules, cfg.routing.resolve_dns)?;
        let rules: Vec<CompiledRule> = rules
            .iter()
            .zip(matchers)
            .enumerate()
            .map(|(i, (r, matcher))| CompiledRule {
                label: r
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("{table} rule #{}", i + 1)),
                named: r.name.is_some(),
                fingerprint: rule_fingerprint(r),
                matcher,
                target: r.target.as_slice().to_vec(),
                bypass_mitm: r.bypass_mitm,
            })
            .collect();
        Ok(Self {
            kind,
            segments: segments(&rules),
            has_url_rules: rules.iter().any(|r| r.matcher.has_url_conditions()),
            has_dns_rules: rules.iter().any(|r| r.matcher.needs_dns()),
            rules,
            default: default.map(|d| d.as_slice().to_vec()),
            inherit,
        })
    }

    /// The first rule that matches, in rule order. Indexed runs answer ASCII hosts from
    /// their maps; any other host scans the run, so results equal [`Self::first_match_linear`].
    fn first_match(&self, ctx: &MatchContext<'_>) -> Option<(usize, &CompiledRule)> {
        let indexable = ctx.host.is_ascii() && !ctx.host.bytes().any(|b| b.is_ascii_uppercase());
        for segment in &self.segments {
            let scan = |start: usize, end: usize| {
                self.rules[start..end]
                    .iter()
                    .position(|r| r.matcher.matches(ctx))
                    .map(|p| start + p)
            };
            let hit = match segment {
                Segment::Linear(start, end) => scan(*start, *end),
                Segment::Run(run) if indexable => run.first_match(&self.rules, ctx),
                Segment::Run(run) => scan(run.start, run.end),
            };
            if let Some(i) = hit {
                return Some((i, &self.rules[i]));
            }
        }
        None
    }

    /// The reference: every rule in order.
    #[cfg(all(test, feature = "routing"))]
    fn first_match_linear(&self, ctx: &MatchContext<'_>) -> Option<(usize, &CompiledRule)> {
        self.rules
            .iter()
            .enumerate()
            .find(|(_, r)| r.matcher.matches(ctx))
    }
}

/// The compiled, immutable routing engine.
#[derive(Debug, Clone)]
pub struct Router {
    unified: Table,
    http: Option<Table>,
    socks: Option<Table>,
    profiles: BTreeMap<String, ProfileConfig>,
    bypass: BTreeMap<String, BypassList>,
    filter: Option<CompiledFilter>,
    notices: Vec<String>,
}

impl Router {
    /// Compile and fully validate the routing model of `cfg`, checking self-loops
    /// against its [`RoutingInput::listeners`].
    pub fn from_input(cfg: &RoutingInput) -> Result<Self, RouterError> {
        Self::from_input_with_listeners(cfg, &cfg.listeners)
    }

    /// Compile and validate against explicit active `listeners` (hot-reload).
    pub fn from_input_with_listeners(
        cfg: &RoutingInput,
        listeners: &[SocketAddr],
    ) -> Result<Self, RouterError> {
        let r = &cfg.routing;
        validate::check_composites(&cfg.profiles)?;
        validate::check_self_loop(&cfg.profiles, listeners)?;
        for (name, p) in &cfg.profiles {
            if !p.bypass.is_empty() && matches!(p.kind, ProfileType::Direct | ProfileType::Block) {
                return Err(RouterError::InvalidProfileBypass {
                    profile: name.clone(),
                    source: condition::ConditionError {
                        field: "bypass",
                        message: format!("is not supported on `{}` profiles", p.kind),
                    },
                });
            }
        }
        let bypass = validate::compile_profile_bypass(&cfg.profiles)?;
        let unified = Table::compile(cfg, TableKind::Unified, &r.rules, Some(&r.default), false)?;
        let split = |kind, t: &Option<crate::routing::RoutingTableConfig>| {
            t.as_ref()
                .map(|t| Table::compile(cfg, kind, &t.rules, t.default.as_ref(), t.inherit))
                .transpose()
        };
        let http = split(TableKind::Http, &r.http)?;
        let socks = split(TableKind::Socks, &r.socks)?;
        Ok(Self {
            unified,
            http,
            socks,
            profiles: cfg.profiles_with_builtins(),
            bypass,
            filter: cfg.filter.clone().map(CompiledFilter::new),
            notices: validate::url_rule_notices(r, cfg.mitm_enabled),
        })
    }

    /// `INFO` notices to log at startup / reload (URL rules on opaque tunnels).
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    /// A profile by name (built-ins included).
    pub fn profile(&self, name: &str) -> Option<&ProfileConfig> {
        self.profiles.get(name)
    }

    /// Every rule label with its fingerprint and whether it is unnamed (positional).
    pub(crate) fn rule_labels(&self) -> HashMap<&str, (u64, bool)> {
        [Some(&self.unified), self.http.as_ref(), self.socks.as_ref()]
            .into_iter()
            .flatten()
            .flat_map(|t| &t.rules)
            .map(|r| (r.label.as_str(), (r.fingerprint, !r.named)))
            .collect()
    }

    /// Number of profiles, built-ins included.
    pub fn profile_count(&self) -> usize {
        self.profiles.len()
    }

    /// Total number of compiled rules across all tables.
    pub fn rule_count(&self) -> usize {
        self.unified.rules.len()
            + self.http.as_ref().map_or(0, |t| t.rules.len())
            + self.socks.as_ref().map_or(0, |t| t.rules.len())
    }

    /// Whether any table consulted for `inbound` traffic has URL rules.
    pub fn has_url_rules(&self, inbound: Inbound) -> bool {
        match self.split_for(inbound) {
            Some(t) => t.has_url_rules || (t.inherit && self.unified.has_url_rules),
            None => self.unified.has_url_rules,
        }
    }

    /// Whether any table consulted for `inbound` traffic has a `cidr`/`ip` rule with
    /// `resolve_dns` on, so routing a hostname may need a DNS lookup.
    pub fn needs_dns(&self, inbound: Inbound) -> bool {
        match self.split_for(inbound) {
            Some(t) => t.has_dns_rules || (t.inherit && self.unified.has_dns_rules),
            None => self.unified.has_dns_rules,
        }
    }

    /// Whether a URL rule in a table consulted for `inbound` traffic could match some
    /// request to `host:port`: `false` when none can, so a `CONNECT` to it can be routed
    /// once instead of per decrypted request. Conservative (see
    /// [`RuleMatcher::may_match_host_port`]); never resolves DNS.
    pub fn host_may_match_url_rules(&self, inbound: Inbound, host: &str, port: u16) -> bool {
        if !self.has_url_rules(inbound) {
            return false;
        }
        let host = normalize_host(host);
        let ip = condition::parse_ip_literal(&host);
        let may_match = |t: &Table| {
            t.has_url_rules
                && t.rules.iter().any(|r| {
                    r.matcher.has_url_conditions() && r.matcher.may_match_host_port(&host, ip, port)
                })
        };
        match self.split_for(inbound) {
            Some(t) => may_match(t) || (t.inherit && may_match(&self.unified)),
            None => may_match(&self.unified),
        }
    }

    fn split_for(&self, inbound: Inbound) -> Option<&Table> {
        match inbound {
            Inbound::Http => self.http.as_ref(),
            Inbound::Socks => self.socks.as_ref(),
        }
    }

    /// Route with the system clock and (opt-in only) the OS resolver.
    pub fn route(&self, req: &RouteRequest<'_>) -> Result<RouteDecision, AccessDenied> {
        self.route_with(req, &SystemClock, &SystemResolver)
    }

    /// Route with the system clock, answering `resolve_dns` lookups from `ips` (already
    /// resolved by the caller, for example asynchronously). Never performs DNS itself.
    pub fn route_with_ips(
        &self,
        req: &RouteRequest<'_>,
        ips: &[IpAddr],
    ) -> Result<RouteDecision, AccessDenied> {
        self.route_with(req, &SystemClock, &SliceResolver(ips))
    }

    /// Route with an injected clock and resolver (deterministic tests, dry runs).
    pub fn route_with(
        &self,
        req: &RouteRequest<'_>,
        clock: &dyn Clock,
        resolver: &dyn Resolver,
    ) -> Result<RouteDecision, AccessDenied> {
        let host = condition::normalize_host_cow(req.host);
        if let Some(f) = &self.filter {
            f.check_host(&host)?;
        }
        let url_scheme = req
            .url
            .and_then(|u| u.split_once("://"))
            .map(|(s, _)| s.to_ascii_lowercase());
        let scheme = req.scheme.map(str::to_ascii_lowercase).or(url_scheme);
        let ctx = MatchContext::new(
            req.inbound,
            &host,
            req.port,
            scheme.as_deref(),
            req.url,
            clock,
            resolver,
        );

        let has_url_rules = self.has_url_rules(req.inbound);
        fn hit<'t>(
            t: &'t Table,
            ctx: &MatchContext<'_>,
        ) -> Option<(TableKind, usize, &'t CompiledRule)> {
            t.first_match(ctx).map(|(i, r)| (t.kind, i, r))
        }
        static DIRECT: &[String] = &[];
        // Defaults are borrowed and only looked at when no rule matched.
        let (table, matched, default): (TableKind, _, &[String]) = match self.split_for(req.inbound)
        {
            Some(t) => {
                let matched =
                    hit(t, &ctx).or_else(|| t.inherit.then(|| hit(&self.unified, &ctx)).flatten());
                let default = match (&t.default, t.inherit) {
                    (Some(d), _) => d.as_slice(),
                    (None, true) => self.unified.default.as_deref().unwrap_or(DIRECT),
                    (None, false) => DIRECT,
                };
                (t.kind, matched, default)
            }
            None => (
                TableKind::Unified,
                hit(&self.unified, &ctx),
                self.unified.default.as_deref().unwrap_or(DIRECT),
            ),
        };

        let (table, rule, rule_index, names, bypass_mitm) = match matched {
            Some((kind, i, r)) => (
                kind,
                Some(r.label.clone()),
                Some(i),
                r.target.as_slice(),
                r.bypass_mitm,
            ),
            None => (table, None, None, default, false),
        };
        // An empty chain (no default anywhere) routes `direct`.
        let targets = if names.is_empty() {
            vec![self.resolve_target(PROFILE_DIRECT, &ctx)]
        } else {
            names.iter().map(|n| self.resolve_target(n, &ctx)).collect()
        };
        Ok(RouteDecision {
            table,
            rule,
            rule_index,
            targets,
            has_url_rules,
            bypass_mitm,
        })
    }

    /// Resolve a chain element through profile-level bypass and composite profiles.
    fn resolve_target(&self, name: &str, ctx: &MatchContext<'_>) -> ResolvedTarget {
        let direct = |bypassed| ResolvedTarget {
            name: name.to_string(),
            profile: PROFILE_DIRECT.to_string(),
            kind: ProfileType::Direct,
            bypassed,
        };
        let mut current = name.to_string();
        // Validation rejects composite cycles; the bound is a belt-and-braces guard.
        for _ in 0..=self.profiles.len() {
            if self.bypass.get(&current).is_some_and(|b| b.matches(ctx)) {
                return direct(true);
            }
            let Some(p) = self.profiles.get(&current) else {
                return direct(false);
            };
            if p.kind != ProfileType::Composite {
                return ResolvedTarget {
                    name: name.to_string(),
                    profile: current,
                    kind: p.kind,
                    bypassed: false,
                };
            }
            // AirSwitch FixedProfile: per-scheme proxy, else fallback, else DIRECT.
            let sub = match ctx.scheme {
                Some("http") => p.http_proxy.as_ref(),
                Some("https") => p.https_proxy.as_ref(),
                _ => None,
            }
            .or(p.fallback_proxy.as_ref());
            match sub {
                Some(s) => current = s.clone(),
                None => return direct(false),
            }
        }
        direct(false)
    }
}

// ---------------------------------------------------------------------------
// test-route target parsing
// ---------------------------------------------------------------------------

/// A `test-route` target: `HOST`, `HOST:PORT`, `[V6]:PORT`, bare IPv6, or a full URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTarget {
    pub host: String,
    pub port: u16,
    /// Lowercase scheme for URL targets.
    pub scheme: Option<String>,
    /// The URL as given, for URL targets.
    pub url: Option<String>,
}

/// Default port for a `HOST` target without `:PORT`.
pub const DEFAULT_TARGET_PORT: u16 = 80;

impl ParsedTarget {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("empty target".into());
        }
        if let Some((scheme, rest)) = raw.split_once("://") {
            let scheme = scheme.to_ascii_lowercase();
            let default_port = match scheme.as_str() {
                "http" => 80,
                "https" => 443,
                other => {
                    return Err(format!(
                        "unsupported URL scheme {other:?} (use http or https)"
                    ))
                }
            };
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            let authority = authority.rsplit('@').next().unwrap_or(authority);
            let (host, port) = split_host_port(authority, default_port)?;
            return Ok(Self {
                host,
                port,
                scheme: Some(scheme),
                url: Some(raw.to_string()),
            });
        }
        let (host, port) = split_host_port(raw, DEFAULT_TARGET_PORT)?;
        Ok(Self {
            host,
            port,
            scheme: None,
            url: None,
        })
    }
}

fn split_host_port(s: &str, default_port: u16) -> Result<(String, u16), String> {
    let parse_port = |p: &str| {
        p.parse::<u16>()
            .map_err(|_| format!("invalid port {p:?} in target {s:?}"))
    };
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, after) = rest
            .split_once(']')
            .ok_or_else(|| format!("unterminated `[` in target {s:?}"))?;
        let port = match after.strip_prefix(':') {
            Some(p) => parse_port(p)?,
            None if after.is_empty() => default_port,
            None => return Err(format!("unexpected {after:?} after `]` in target {s:?}")),
        };
        (h.to_string(), port)
    } else if s.matches(':').count() > 1 {
        // Bare IPv6 literal without a port.
        (s.to_string(), default_port)
    } else if let Some((h, p)) = s.rsplit_once(':') {
        (h.to_string(), parse_port(p)?)
    } else {
        (s.to_string(), default_port)
    };
    if host.is_empty() {
        return Err(format!("target {s:?} has no host"));
    }
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::test_support;

    fn router(text: &str) -> Router {
        Router::from_input(&test_support::input(text)).unwrap()
    }

    const NOON: FixedClock = FixedClock(LocalTime {
        weekday: 0,
        minute_of_day: 720,
    });

    /// Generated tables: indexed `first_match` must equal the linear reference.
    #[cfg(feature = "routing")]
    #[test]
    fn indexed_first_match_equals_linear_scan() {
        let next = |state: &mut u64| {
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *state >> 33
        };
        let labels = ["a", "b", "corp", "ex", "lan", "dev-1", "x"];
        for seed in [1u64, 2, 3] {
            let mut state = seed;
            let name = |state: &mut u64| -> String {
                let n = 1 + next(state) % 3;
                (0..n)
                    .map(|_| labels[(next(state) % labels.len() as u64) as usize])
                    .collect::<Vec<_>>()
                    .join(".")
            };
            let mut text =
                String::from("[profiles.p]\ntype = \"http\"\nhost = \"h\"\nport = 1\n\n");
            for i in 0..1200u64 {
                let n = name(&mut state);
                let body = match next(&mut state) % 14 {
                    0..=3 => format!("host = \"{n}\""),
                    4..=6 => format!("host_wildcard = \"*.{n}\""),
                    7 => format!("host_wildcard = \"**.{n}\""),
                    8 => format!("host_wildcard = [\"{n}\", \"dev-*.{n}\"]"),
                    9 => format!("host = \"{n}\"\ndisabled = true"),
                    10 if i % 97 == 0 => "host_wildcard = \"*\"".to_string(),
                    10 => format!("host_wildcard = \"h?st.{n}\""),
                    11 if i % 151 == 0 => "match_all = true".to_string(),
                    11 => format!("host = \"{n}\"\nport = 443"),
                    12 => format!("host_regex = \"^{n}$\""),
                    _ => format!("host = [\"{n}\", \"z.{n}\", \"y.{n}\"]"),
                };
                text.push_str(&format!("[[routing.rules]]\n{body}\ntarget = \"p\"\n\n"));
            }
            let r = router(&text);
            assert!(
                r.unified
                    .segments
                    .iter()
                    .any(|s| matches!(s, Segment::Run(_))),
                "the generated table must contain indexed runs"
            );
            let mut hosts: Vec<String> = (0..3000).map(|_| name(&mut state)).collect();
            hosts.extend(
                [
                    "host.lan",
                    "hast.corp",
                    "dev-1.a",
                    "k.\u{212a}.lan",
                    "",
                    "q",
                ]
                .map(String::from),
            );
            for host in &hosts {
                for port in [80u16, 443] {
                    let ctx = MatchContext::new(
                        Inbound::Http,
                        host,
                        port,
                        None,
                        None,
                        &NOON,
                        &SliceResolver(&[]),
                    );
                    assert_eq!(
                        r.unified.first_match(&ctx).map(|(i, _)| i),
                        r.unified.first_match_linear(&ctx).map(|(i, _)| i),
                        "seed {seed} host {host:?} port {port}"
                    );
                }
            }
        }
    }

    fn route(r: &Router, inbound: Inbound, host: &str, port: u16) -> RouteDecision {
        r.route_with(
            &RouteRequest::new(inbound, host, port),
            &NOON,
            &SystemResolver,
        )
        .unwrap()
    }

    #[cfg(feature = "routing")]
    const PROFILES: &str = r#"
        [profiles.corp]
        type = "http"
        host = "10.0.0.2"
        port = 3128
        bypass = ["127.0.0.1", "*.local"]
        [profiles.bastion]
        type = "socks5"
        host = "127.0.0.1"
        port = 1081
        [profiles.secure]
        type = "https"
        host = "proxy.example"
        port = 8443
    "#;

    #[cfg(feature = "routing")]
    #[test]
    fn unified_top_to_bottom_with_default() {
        let r = router(&format!(
            "{PROFILES}
            [routing]
            default = [\"corp\", \"direct\"]
            [[routing.rules]]
            name = \"first\"
            host_wildcard = \"*.example.com\"
            target = \"bastion\"
            [[routing.rules]]
            name = \"second\"
            host = \"www.example.com\"
            target = \"block\"
            "
        ));
        let d = route(&r, Inbound::Http, "www.example.com", 443);
        assert_eq!(d.table, TableKind::Unified);
        assert_eq!(d.rule_label(), "first");
        assert_eq!(d.rule_index, Some(0));
        assert_eq!(d.profile_label(), "bastion");
        let d = route(&r, Inbound::Socks, "other.test", 22);
        assert_eq!(d.rule_label(), "<default>");
        assert_eq!(d.chain_label(), "corp,direct");
        assert!(!d.has_url_rules);
        assert_eq!(r.rule_count(), 2);
        assert_eq!(r.profile_count(), 5);
    }

    #[cfg(feature = "routing")]
    #[test]
    fn split_tables_with_and_without_inherit() {
        let base = format!(
            "{PROFILES}
            [routing]
            default = \"corp\"
            [[routing.rules]]
            name = \"unified-db\"
            host = \"db.internal\"
            target = \"bastion\"
            "
        );
        let r = router(&format!(
            "{base}
            [routing.socks]
            default = \"secure\"
            [[routing.socks.rules]]
            name = \"socks-local\"
            host_wildcard = \"*.local\"
            target = \"direct\"
            "
        ));
        let d = route(&r, Inbound::Socks, "nas.local", 445);
        assert_eq!((d.table, d.rule_label()), (TableKind::Socks, "socks-local"));
        // No inherit: unified rules are not consulted for SOCKS.
        let d = route(&r, Inbound::Socks, "db.internal", 5432);
        assert_eq!((d.table, d.rule_label()), (TableKind::Socks, "<default>"));
        assert_eq!(d.profile_label(), "secure");
        // HTTP has no split table: unified.
        let d = route(&r, Inbound::Http, "db.internal", 443);
        assert_eq!(
            (d.table, d.profile_label().as_str()),
            (TableKind::Unified, "bastion")
        );

        let r = router(&format!(
            "{base}
            [routing.http]
            inherit = true
            [[routing.http.rules]]
            name = \"http-only\"
            port = 80
            target = \"block\"
            "
        ));
        let d = route(&r, Inbound::Http, "db.internal", 80);
        assert_eq!(d.rule_label(), "http-only");
        let d = route(&r, Inbound::Http, "db.internal", 443);
        assert_eq!(
            (d.table, d.rule_label()),
            (TableKind::Unified, "unified-db")
        );
        let d = route(&r, Inbound::Http, "x.test", 443);
        assert_eq!(
            (d.table, d.profile_label().as_str()),
            (TableKind::Http, "corp")
        );

        // Split table without default and without inherit falls back to direct.
        let r = router(&format!("{base}\n[routing.http]\n"));
        assert_eq!(
            route(&r, Inbound::Http, "x.test", 443).profile_label(),
            "direct"
        );
    }

    #[cfg(feature = "routing")]
    #[test]
    fn profile_bypass_and_composites() {
        let r = router(&format!(
            "{PROFILES}
            [profiles.split]
            type = \"composite\"
            http_proxy = \"corp\"
            https_proxy = \"secure\"
            [profiles.fb]
            type = \"composite\"
            fallback_proxy = \"bastion\"
            bypass = [\"<local>\"]
            [routing]
            default = [\"corp\", \"split\", \"fb\"]
            "
        ));
        let d = route(&r, Inbound::Http, "printer.local", 631);
        assert_eq!(d.targets[0].label(), "corp(bypass->direct)");
        assert_eq!(d.targets[0].kind, ProfileType::Direct);
        assert!(d.targets[0].bypassed);
        // No scheme: composite without fallback goes direct; fb uses its fallback.
        assert_eq!(d.targets[1].profile, "direct");
        assert_eq!(d.targets[2].label(), "fb->bastion");

        let req = RouteRequest {
            scheme: None,
            url: Some("http://site.example/x"),
            ..RouteRequest::new(Inbound::Http, "site.example", 80)
        };
        let d = r.route_with(&req, &NOON, &SystemResolver).unwrap();
        assert_eq!(d.targets[1].label(), "split->corp");
        let req = RouteRequest {
            scheme: Some("HTTPS"),
            ..RouteRequest::new(Inbound::Http, "site.example", 443)
        };
        let d = r.route_with(&req, &NOON, &SystemResolver).unwrap();
        assert_eq!(d.targets[1].label(), "split->secure");
        // fb's own bypass list: dot-less host.
        let d = route(&r, Inbound::Socks, "intranet", 80);
        assert_eq!(d.targets[2].label(), "fb(bypass->direct)");
    }

    #[test]
    fn global_filter_runs_first() {
        let r = router("[filter]\nblock = [\"*.ads.example\"]\n");
        let e = r
            .route_with(
                &RouteRequest::new(Inbound::Http, "x.ads.example", 443),
                &NOON,
                &SystemResolver,
            )
            .unwrap_err();
        assert!(e.0.contains("blocklisted"));
        assert!(r
            .route_with(
                &RouteRequest::new(Inbound::Http, "ok.example", 443),
                &NOON,
                &SystemResolver
            )
            .is_ok());
    }

    #[test]
    fn has_url_rules_follows_active_tables() {
        let r = router(
            "[[routing.rules]]\nkeyword = \"k\"\ntarget = \"direct\"\n\
             [routing.socks]\n[routing.http]\ninherit = true\n",
        );
        assert!(r.has_url_rules(Inbound::Http));
        assert!(!r.has_url_rules(Inbound::Socks));
        assert!(route(&r, Inbound::Http, "h", 1).has_url_rules);
    }

    #[test]
    fn needs_dns_follows_resolve_dns_and_active_tables() {
        let r = router("[[routing.rules]]\ncidr = \"10.0.0.0/8\"\ntarget = \"direct\"\n");
        assert!(!r.needs_dns(Inbound::Http));
        let r = router(
            "[[routing.rules]]\ncidr = \"10.0.0.0/8\"\nresolve_dns = true\ntarget = \"direct\"\n",
        );
        assert!(r.needs_dns(Inbound::Http));
        assert!(r.needs_dns(Inbound::Socks));
        // The global switch applies; a disabled rule never needs DNS.
        let r = router(
            "[routing]\nresolve_dns = true\n[[routing.rules]]\ncidr = \"10.0.0.0/8\"\n\
             target = \"direct\"\n",
        );
        assert!(r.needs_dns(Inbound::Http));
        let r = router(
            "[[routing.rules]]\ncidr = \"10.0.0.0/8\"\nresolve_dns = true\ndisabled = true\n\
             target = \"direct\"\n",
        );
        assert!(!r.needs_dns(Inbound::Http));
        // Split tables: unified DNS rules count only with `inherit`.
        let r = router(
            "[[routing.rules]]\ncidr = \"10.0.0.0/8\"\nresolve_dns = true\ntarget = \"direct\"\n\
             [routing.socks]\n[routing.http]\ninherit = true\n",
        );
        assert!(r.needs_dns(Inbound::Http));
        assert!(!r.needs_dns(Inbound::Socks));
    }

    #[test]
    fn route_with_ips_answers_resolve_dns_from_slice() {
        let r = router(
            "[profiles.p]\ntype = \"socks5\"\nhost = \"127.0.0.1\"\nport = 1081\n\
             [[routing.rules]]\nname = \"lan\"\ncidr = \"10.0.0.0/8\"\nresolve_dns = true\n\
             target = \"p\"\n",
        );
        let req = RouteRequest::new(Inbound::Http, "lan.example", 443);
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let d = r.route_with_ips(&req, &[ip("10.1.2.3")]).unwrap();
        assert_eq!(d.rule.as_deref(), Some("lan"));
        assert_eq!(d.profile_label(), "p");
        // IPv4-mapped IPv6 is canonicalized before matching.
        let d = r.route_with_ips(&req, &[ip("::ffff:10.1.2.3")]).unwrap();
        assert_eq!(d.rule.as_deref(), Some("lan"));
        let d = r.route_with_ips(&req, &[ip("192.168.0.1")]).unwrap();
        assert_eq!(d.rule, None);
        let d = r.route_with_ips(&req, &[]).unwrap();
        assert_eq!(d.rule, None);
    }

    #[test]
    fn host_may_match_url_rules_scopes_to_rule_hosts() {
        // No URL rules at all.
        let r = router("[[routing.rules]]\nhost = \"a.test\"\ntarget = \"direct\"\n");
        assert!(!r.host_may_match_url_rules(Inbound::Http, "a.test", 443));

        // A URL rule scoped to one host (and port) only.
        let r = router(
            "[[routing.rules]]\nhost = \"a.test\"\nport = \"443\"\nkeyword = \"k\"\n\
             target = \"direct\"\n",
        );
        assert!(r.has_url_rules(Inbound::Http));
        assert!(r.host_may_match_url_rules(Inbound::Http, "a.test", 443));
        assert!(r.host_may_match_url_rules(Inbound::Http, "A.Test.", 443));
        assert!(!r.host_may_match_url_rules(Inbound::Http, "b.test", 443));
        assert!(!r.host_may_match_url_rules(Inbound::Http, "a.test", 8443));

        // An unscoped URL rule may match any host.
        let r = router("[[routing.rules]]\nkeyword = \"k\"\ntarget = \"direct\"\n");
        for host in ["a.test", "b.test", "10.0.0.1", "::1"] {
            assert!(
                r.host_may_match_url_rules(Inbound::Http, host, 443),
                "{host}"
            );
        }

        // CIDR-scoped: IP literals are checked, hostnames only match with `resolve_dns`.
        let r = router(
            "[[routing.rules]]\ncidr = \"10.0.0.0/8\"\nkeyword = \"k\"\ntarget = \"direct\"\n",
        );
        assert!(r.host_may_match_url_rules(Inbound::Http, "10.1.2.3", 443));
        assert!(!r.host_may_match_url_rules(Inbound::Http, "192.168.0.1", 443));
        assert!(!r.host_may_match_url_rules(Inbound::Http, "a.test", 443));
        let r = router(
            "[[routing.rules]]\ncidr = \"10.0.0.0/8\"\nresolve_dns = true\nkeyword = \"k\"\n\
             target = \"direct\"\n",
        );
        assert!(r.host_may_match_url_rules(Inbound::Http, "a.test", 443));
        assert!(!r.host_may_match_url_rules(Inbound::Http, "192.168.0.1", 443));
    }

    #[test]
    fn host_may_match_url_rules_follows_active_tables() {
        let r = router(
            "[[routing.rules]]\nhost = \"a.test\"\nkeyword = \"k\"\ntarget = \"direct\"\n\
             [routing.socks]\n[routing.http]\ninherit = true\n\
             [[routing.http.rules]]\nhost = \"b.test\"\nkeyword = \"k\"\ntarget = \"direct\"\n",
        );
        assert!(r.host_may_match_url_rules(Inbound::Http, "a.test", 443));
        assert!(r.host_may_match_url_rules(Inbound::Http, "b.test", 443));
        assert!(!r.host_may_match_url_rules(Inbound::Http, "c.test", 443));
        assert!(!r.host_may_match_url_rules(Inbound::Socks, "a.test", 443));
        // Without `inherit`, unified URL rules are not consulted.
        let r = router(
            "[[routing.rules]]\nhost = \"a.test\"\nkeyword = \"k\"\ntarget = \"direct\"\n\
             [routing.http]\n\
             [[routing.http.rules]]\nhost = \"b.test\"\nkeyword = \"k\"\ntarget = \"direct\"\n",
        );
        assert!(!r.host_may_match_url_rules(Inbound::Http, "a.test", 443));
        assert!(r.host_may_match_url_rules(Inbound::Http, "b.test", 443));
    }

    #[test]
    fn invalid_bypass_on_builtin_kinds() {
        let cfg = test_support::input("[profiles.direct]\ntype = \"direct\"\nbypass = [\"x\"]\n");
        assert!(Router::from_input(&cfg)
            .unwrap_err()
            .to_string()
            .contains("not supported on `direct`"));
    }

    #[test]
    fn parse_targets() {
        let t = |s: &str| ParsedTarget::parse(s).unwrap();
        assert_eq!((t("db:5432").host, t("db:5432").port), ("db".into(), 5432));
        assert_eq!(t("example.com").port, DEFAULT_TARGET_PORT);
        assert_eq!(t("[::1]:22").host, "::1");
        assert_eq!(t("[::1]:22").port, 22);
        assert_eq!(t("fd00::1").host, "fd00::1");
        let u = t("https://user@API.example:8443/v1?q#f");
        assert_eq!(
            (u.host.as_str(), u.port, u.scheme.as_deref()),
            ("API.example", 8443, Some("https"))
        );
        assert_eq!(t("http://h/").port, 80);
        assert_eq!(t("https://[fd00::2]/").host, "fd00::2");
        assert!(ParsedTarget::parse("ftp://h").is_err());
        assert!(ParsedTarget::parse("h:99999").is_err());
        assert!(ParsedTarget::parse("").is_err());
        assert!(ParsedTarget::parse(":80").is_err());
    }
}
