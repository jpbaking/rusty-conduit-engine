# phase-0001 — MITM bypass normalization, H2 stream caps/adaptive window, TLS session reuse, and host-scoped URL-rule check

- Status: done
- Owner: executor
- Depends on: none
- Goal: Fix MITM bypass hostname normalization (`B-01`), cap MITM H2 concurrent streams (`B-06`) and enable adaptive H2 flow control (`P-10`), reuse MITM TLS server session storage/ticketer and pre-built upstream ALPN `ClientConfig`s (`P-05`), and scope MITM `per_request` downgrade to hosts that can actually match a URL rule (`P-10`).
- Done when: every command in `## Verify` exits 0.
- Pattern to follow: `src/filter.rs:325` (`normalize_domain`) and `src/http/mitm_tunnel.rs:621-634` (`should_bypass`)

## Context
`MitmInterceptor::should_bypass` strips only `[...]` brackets without lowercasing or stripping trailing dots, so `CONNECT Example.com:443` or `CONNECT example.com.:443` misses `--mitm-bypass-host`.
In `src/http/mitm_tunnel.rs`, `server_http2::Builder` leaves `max_concurrent_streams` unbounded and omits `.adaptive_window(true)` (as does `client_http2::Builder`). Each MITM `CONNECT` builds a fresh `ServerConfig` with a new default 32-shard session cache and ticketer, breaking TLS session resumption, and `Dialer::connect` clones `ClientConfig` on every upstream dial just to set `alpn_protocols`.
Finally, `state.router.has_url_rules(Inbound::Http)` forces every MITM `CONNECT` into HTTP/1.1 `per_request` mode even when the only URL rule is scoped to one specific host.

## Scope
- In: `src/http/mitm_tunnel.rs`, `src/router/mod.rs`, `src/router/condition.rs` (helper method on `RuleMatcher` if needed for host-scoped URL-rule check)
- Out: `src/server.rs`, `src/net.rs`, `src/upstream/`, `src/mitm/cache.rs`, `src/logging.rs`, `docs/`

## Sub-tasks
1. [done] In `src/http/mitm_tunnel.rs` (`MitmInterceptor::should_bypass`), normalize non-IP `trimmed` via `crate::filter::normalize_domain(trimmed)` before calling `matches_domain`. Add unit tests in `src/http/mitm_tunnel.rs` proving uppercase (`Example.COM`) and trailing-dot (`example.com.`) bypass rules match — done when: `cargo test --locked should_bypass` passes.
2. [done] In `src/http/mitm_tunnel.rs`, add `const H2_MAX_CONCURRENT_STREAMS: u32 = 256;`, set `.max_concurrent_streams(H2_MAX_CONCURRENT_STREAMS)` and `.adaptive_window(true)` on `server_http2::Builder` in `serve_h2`, and set `.adaptive_window(true)` on `client_http2::Builder` in `h2_handshake` — done when: `grep -c 'max_concurrent_streams' src/http/mitm_tunnel.rs` is at least 1 and `grep -c 'adaptive_window(true)' src/http/mitm_tunnel.rs` is at least 2.
3. [done] In `src/http/mitm_tunnel.rs`, store shared `session_storage: Arc<dyn rustls::server::StoresServerSessions>` (`rustls::server::ServerSessionMemoryCache::new(4096)`) and `ticketer: Option<Arc<dyn rustls::server::ProducesTickets>>` (`rustls::crypto::ring::Ticketer::new().ok()`) on `MitmInterceptor` and attach them to every `ServerConfig` built in `intercept`. Also pre-build the 4 ALPN variants of `Arc<ClientConfig>` (`[]`, `[http/1.1]`, `[h2]`, `[h2, http/1.1]`) once in `MitmInterceptor::new` (in a small `UpstreamTlsPool` struct cloned into `Dialer`) so `Dialer::connect` reuses a pre-built `Arc<ClientConfig>` without cloning `ClientConfig` on the common ALPN combinations — done when: `cargo test --locked mitm` passes.
4. [done] In `src/router/condition.rs` (add `RuleMatcher::may_match_host_port(&self, host: &str, ip: Option<IpAddr>, port: u16) -> bool`) and `src/router/mod.rs`, add `pub fn host_may_match_url_rules(&self, inbound: Inbound, host: &str, port: u16) -> bool`: return `false` if `!self.has_url_rules(inbound)`; otherwise return `true` if any URL-bearing rule in the active table(s) for `inbound` has no disqualifying host/port constraint for `(normalize_host(host), port)` (conservatively return `true` for URL rules with no host/port constraint or with `cidr` + `resolve_dns`). Update `MitmInterceptor::intercept` (`src/http/mitm_tunnel.rs:709`) to call `state.router.host_may_match_url_rules(Inbound::Http, &target.host, target.port)`. Add unit tests in `src/router/mod.rs` covering host-scoped vs unscoped URL rules — done when: `cargo test --locked host_may_match_url_rules` passes.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
```

## Escalate when
- `rustls::crypto::ring::Ticketer` or `ServerSessionMemoryCache` is not available under the pinned `rustls` 0.23 feature set → stop, set Status: needs-human — reason: paste the compiler error.
- Any existing MITM integration test (`tests/mitm_counters.rs`, `tests/mitm_h2_upstream_reconnect.rs`) fails unexpectedly → stop, set Status: needs-human — reason: paste the failing test output.

## Log
- (append-only, one line per event)
- sub-task 1: `should_bypass` normalizes non-IP hosts with `crate::filter::normalize_domain`; added test `should_bypass_normalizes_case_and_trailing_dot`
- sub-task 2: added `H2_MAX_CONCURRENT_STREAMS = 256`; server H2 builder sets `max_concurrent_streams` + `adaptive_window(true)`, client H2 builder sets `adaptive_window(true)`
- sub-task 3: `MitmInterceptor` holds shared `ServerSessionMemoryCache::new(4096)` + `ring::Ticketer::new().ok()` attached to every intercept `ServerConfig`; new `UpstreamTlsPool` pre-builds 4 ALPN `Arc<ClientConfig>` variants, cloned into `Dialer`, uncommon orders fall back to a fresh clone; added tests `upstream_tls_pool_reuses_common_alpn_configs`, `interceptor_shares_server_session_storage_and_ticketer`
- sub-task 4: added `RuleMatcher::may_match_host_port` (condition.rs) and `Router::host_may_match_url_rules` (mod.rs, honours split tables + `inherit`); `MitmInterceptor::intercept` now uses it for `per_request`; added tests `host_may_match_url_rules_scopes_to_rule_hosts`, `host_may_match_url_rules_follows_active_tables`
- ran `cargo fmt` to format the new tests, then the full `## Verify` block; all passed

## Evidence
- (append-only: command → observed result)
- `cargo test --locked should_bypass` → `test http::mitm_tunnel::tests::should_bypass_normalizes_case_and_trailing_dot ... ok` (1 passed)
- `grep -c max_concurrent_streams src/http/mitm_tunnel.rs` → 1; `grep -c "adaptive_window(true)" src/http/mitm_tunnel.rs` → 2; `cargo check --locked` → Finished
- `cargo test --locked mitm` → lib 43 passed, error_pages 2 passed, 4 passed (tests/mitm_*), 0 failed
- `cargo test --locked host_may_match_url_rules` → 2 passed, 0 failed
- `## Verify`: `cargo fmt --check` → exit 0; `cargo clippy --all-targets --locked -- -D warnings` → exit 0; `cargo clippy --all-targets --no-default-features --locked -- -D warnings` → exit 0; `cargo test --locked` → exit 0, 200 passed / 0 failed; `cargo test --no-default-features --locked` → exit 0, 188 passed / 0 failed
