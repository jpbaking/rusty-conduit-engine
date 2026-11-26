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
- IP/CIDR conditions never do DNS lookups unless `resolve_dns` is on. Even then, look up lazily, at most once per decision.
- Regex-based conditions (`host_regex`, `url_regex`, `host_wildcard`, `url_wildcard`, wildcard `bypass`) need the `routing` cargo feature. Without it they must fail at compile time with an error naming the feature, and every other condition must keep working.
- Validation runs at startup, in `validate`, and before every hot-reload swap. Any error rejects the candidate configuration.

## Feature Map
- **Route decision** — `Router::route`: filter, table selection, rule evaluation, and failover-chain and composite/bypass profile resolution. Start: `mod.rs`.
- **Rule conditions** — compiled matchers for host, URL, IP/CIDR, port, host levels, weekday/time, keyword, inbound, and bypass, plus the `Clock` and `Resolver` hooks. Start: `condition.rs`.
- **Routing validation** — profile references, failover targets, composite cycles, rule syntax, self-loop detection, URL-rule notices. Start: `validate.rs`.

## Child DOX Index
- (none)
