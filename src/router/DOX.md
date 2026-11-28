# src/router

## Purpose
Auto Switch routing engine: compiles rules into an immutable `Router`, matches each `RouteRequest` to a decision, and validates routing configuration.

## Ownership
Owns code under `src/router/`. The serde routing/profile model, `RoutingState` snapshots, and `RouteResolver` live in ../routing.rs. The global destination filter is ../filter.rs. Dialers are ../upstream.

## Local Contracts
- A `Router` is compiled once from a validated `RoutingInput` (at startup and on every hot-reload) and is immutable afterwards.
- Routing order: global filter first (denial returns `AccessDenied`), then table selection (`[routing.http]`/`[routing.socks]` split tables, else unified `[routing]`), then rules strictly top to bottom, then the table default. Split-table fallback follows the `inherit` rules documented in `mod.rs`.
- Within a rule: AND across fields, OR within a field.
- Wildcard semantics follow AirSwitch / SwitchyOmega. `bypass` uses Chromium bypass-list syntax.
- IP/CIDR conditions never do DNS lookups unless `resolve_dns` is on. Even then, look up lazily, at most once per decision. The engines avoid the blocking lookup: when `Router::needs_dns` is true they resolve asynchronously first (`resolve_route_async` in ../routing.rs, never cached) and route with `Router::route_with_ips`, which never performs DNS itself.
- `Router::host_may_match_url_rules` must stay conservative: it may answer `true` for a host no URL rule matches, never `false` for one that some URL rule could match. It never resolves DNS.
- Regex-based conditions (`host_regex`, `url_regex`, `host_wildcard`, `url_wildcard`, wildcard `bypass`) need the `routing` cargo feature. Without it they must fail at compile time with an error naming the feature, and every other condition must keep working.
- The first-match index must give exactly the linear scan's answer: rules are evaluated in order, host-only runs (`RuleMatcher::host_only`, at least `MIN_INDEXED_RUN` rules) are answered from their maps for lowercase ASCII hosts and scanned otherwise, and any pattern the fast paths cannot classify stays in a residual regex. `first_match_linear` and `HostWildcardMatcher`'s full regex are the test references; keep the equivalence tests passing.
- Validation runs at startup, in `validate`, and before every hot-reload swap. Any error rejects the candidate configuration.

## Feature Map
- **Route decision** — `Router::route` / `route_with_ips`: filter, table selection, rule evaluation, and failover-chain and composite/bypass profile resolution; `needs_dns` and `host_may_match_url_rules` let callers pre-resolve DNS and skip per-request MITM routing. Start: `mod.rs`.
- **First-match index** — `HostRun` maps (exact, `*.d`, `**.d`, `*`, residual rules) over runs of host-only rules; `Table::first_match` walks segments in order. Start: `mod.rs`.
- **Rule conditions** — compiled matchers (`NameSet`, `HostWildcardMatcher`, combined `host_regex` / `url_regex`) for host, URL, IP/CIDR, port, host levels, weekday/time, keyword, inbound, and bypass, plus the `Clock` and `Resolver` hooks, `needs_dns`, and `may_match_host_port`. Start: `condition.rs`.
- **Routing validation** — profile references, failover targets, composite cycles, rule syntax, self-loop detection, URL-rule notices. Start: `validate.rs`.

## Child DOX Index
- (none)
