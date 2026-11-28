# phase-0004 — Async DNS resolution for resolve_dns routing rules and O(1) CertCache eviction

- Status: done
- Owner: executor
- Depends on: phase-0003
- Goal: Resolve hostnames asynchronously via `tokio::net::lookup_host` before routing when `resolve_dns = true` rules are active (`P-04`), and replace `CertCache`'s `O(N)` eviction scan with an `O(1)` FIFO queue (`P-10`).
- Done when: every command in `## Verify` exits 0.
- Pattern to follow: `src/router/mod.rs:300-345` (`has_url_rules`, `route_with`) and `src/mitm/cache.rs:152-191` (`store_at`, `evict_oldest`)

## Context
`SystemResolver::resolve` in `src/router/condition.rs:180-187` calls blocking `to_socket_addrs()` directly on Tokio worker threads whenever a CIDR rule has `resolve_dns = true`. Adding `Router::needs_dns(inbound)` and `Router::route_with_ips(req, &ips)` lets the async HTTP and SOCKS handlers resolve hostnames via `tokio::net::lookup_host` (bounded by `connect_timeout`) before routing, without changing the synchronous `Router::route` / `RouteResolver::resolve` APIs used by `test-route` and custom resolvers.
In `src/mitm/cache.rs`, `CertCache` has a fixed `ttl`, so insertion order `(expires_at, seq)` is monotonically non-decreasing. Tracking `(seq, host)` in a `VecDeque` inside `insert_lock` lets `evict_oldest` pop the oldest live entry in amortized `O(1)` instead of scanning every `DashMap` entry with `min_by_key`.

## Scope
- In: `src/router/condition.rs`, `src/router/mod.rs`, `src/routing.rs`, `src/http/mod.rs`, `src/http/forward.rs`, `src/http/tunnel.rs`, `src/http/mitm_tunnel.rs`, `src/socks/mod.rs`, `src/mitm/cache.rs`
- Out: `src/server.rs`, `src/net.rs`, `src/upstream/`, `src/logging.rs`, `docs/`

## Sub-tasks
1. [done] In `src/router/condition.rs` and `src/router/mod.rs`, add `RuleMatcher::needs_dns(&self) -> bool` (true if any condition is `Cidr { resolve_dns: true, .. }`), precompute `has_dns_rules: bool` on `Table` (matching `has_url_rules`), and add `pub fn needs_dns(&self, inbound: Inbound) -> bool` and `pub fn route_with_ips(&self, req: &RouteRequest<'_>, ips: &[IpAddr]) -> Result<RouteDecision, AccessDenied>` on `Router` (using a lightweight slice-backed `Resolver` that returns `ips.iter().copied().map(canonical_ip).collect()`). Keep `Router::route` and `Router::route_with` unchanged. Add unit tests in `src/router/mod.rs` — done when: `cargo test --locked needs_dns` and `cargo test --locked route_with_ips` pass.
2. [done] In `src/routing.rs`, add an async helper `pub async fn resolve_route_async(resolver: &dyn RouteResolver, req: &RouteRequest<'_>, dns_timeout: Duration) -> Result<RouteDecision, AccessDenied>` (or method on `RouteResolver` with a default impl / free function in `routing.rs`): when `resolver.state().router.needs_dns(req.inbound)` is `true`, `req.host` is not an IP literal (after stripping `[...]`), and `resolver` is using the built-in router (check via a `fn resolve_with_ips(&self, req: &RouteRequest<'_>, ips: &[IpAddr]) -> Option<Result<RouteDecision, AccessDenied>>` method on `RouteResolver` whose default returns `None` and whose `RoutingState`/`PinnedState`/`SharedRouting` impls return `Some(self.router.route_with_ips(req, ips))`), resolve `(host, req.port)` via `tokio::time::timeout(dns_timeout, tokio::net::lookup_host((host, req.port)))` (falling back to `&[]` on error/timeout, matching `SystemResolver`) and call `resolve_with_ips`; otherwise call `resolver.resolve(req)`. Wire HTTP (`src/http/mod.rs`, `forward.rs`, `tunnel.rs`, `mitm_tunnel.rs`) and SOCKS (`src/socks/mod.rs`) call sites to use `resolve_route_async` with `state.connect_timeout`. Never cache DNS results — done when: `cargo test --locked route_resolver` and `cargo test --locked` pass.
3. [done] In `src/mitm/cache.rs`, replace `insert_lock: Mutex<u64>` with `insert_lock: Mutex<InsertState>` holding `{ next_seq: u64, order: VecDeque<(u64, String)> }`. On `store_at`, push `(seq, host.to_string())` onto `order`; in `evict_oldest` (taking `&mut InsertState`), pop from the front of `order`, skipping stale entries where `self.entries.get(&host)` is absent or has a different `seq`, and remove the first matching live entry in amortized `O(1)`. Also prune `order` front entries whose `expires_at <= now` during `reap_at` or cap enforcement so `order` cannot grow unboundedly across re-inserts. Add a unit test verifying cap eviction order and re-insertion of an existing host — done when: `cargo test --locked mitm::cache` passes.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
```

## Escalate when
- Adding a defaulted method to `RouteResolver` breaks any downstream mock or test in `tests/route_resolver.rs` → stop, set Status: needs-human — reason: paste the compiler error.
- `CertCache::reap_at` is called concurrently with `store_at` and lock ordering could deadlock → keep `insert_lock` strictly outside `DashMap` shard guards (never hold a `DashMap` guard across `insert_lock.lock()`); if unsure, stop, set Status: needs-human — reason: describe the lock ordering issue.

## Log
- (append-only, one line per event)
- sub-task 1: added RuleMatcher::needs_dns (condition.rs), Table.has_dns_rules, Router::needs_dns, Router::route_with_ips with private SliceResolver (mod.rs), plus 2 unit tests
- sub-task 2: added RouteResolver::resolve_with_ips (default None; Some(route_with_ips) on RoutingState, ArcSwap<RoutingState>, SharedRouting, PinnedState) and resolve_route_async (tokio lookup_host bounded by timeout, empty on error, no cache) in routing.rs; wired http/mod.rs, forward.rs, tunnel.rs, mitm_tunnel.rs (Dialer::route and decide now async), socks/mod.rs with state.connect_timeout; 2 unit tests
- sub-task 3: CertCache insert_lock now Mutex<InsertState{next_seq, order: VecDeque<(u64,String)>}>; store_at pushes (seq,host) and drops stale front items; evict_oldest pops FIFO skipping stale (amortized O(1)); reap_at locks insert_lock then reap_locked (retain + prune expired/stale front); lock order insert_lock before DashMap guards, no guard held across lock; 2 new unit tests
- Verify block passed on first run; phase closed

## Evidence
- (append-only: command → observed result)
- `cargo test --locked needs_dns` → router::tests::needs_dns_follows_resolve_dns_and_active_tables ok (1 passed); `cargo test --locked route_with_ips` → router::tests::route_with_ips_answers_resolve_dns_from_slice ok (1 passed)
- `cargo test --locked route_resolver` → tests/route_resolver.rs 2 passed; `cargo test --locked` → all suites ok (lib 194 passed + 2+1+2+4+5+6 integration), 0 failed
- `cargo test --locked mitm::cache` → 15 passed, 0 failed (incl. new cap_evicts_in_insertion_order_after_reinsertion, reinserts_and_reaps_keep_the_queue_bounded)
- Verify: `cargo fmt --check` → exit 0; `cargo clippy --all-targets --locked -- -D warnings` → exit 0, no warnings; `cargo clippy --all-targets --no-default-features --locked -- -D warnings` → exit 0, no warnings; `cargo test --locked` → exit 0 (lib 196 + 5+4+6+2+1 integration passed, 0 failed); `cargo test --no-default-features --locked` → exit 0 (lib 185 + 5+4+6+2 passed, 0 failed)
