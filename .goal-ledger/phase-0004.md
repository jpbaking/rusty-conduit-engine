# phase-0004 — E2a: Add boxed streams, external dialers, RouteResolver trait, and typed dial errors

- Status: done
- Owner: executor
- Depends on: phase-0003
- Goal: Implement Master Plan §4.1 (boxed upstream streams), §4.2 (external dialers), §4.3 (`RouteResolver` trait), and §4.6 (typed dial errors) in `rusty-conduit-engine` with unit and integration tests.
- Done when: All four extension points are implemented with tests covering each, `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py` passes, and `../rusty-conduit-switch` tests still pass.
- Pattern to follow: `src/upstream/mod.rs:50-120`, `src/upstream/block.rs:1-40`, `src/routing.rs`, `src/server.rs`

## Context
This is the first half of Milestone E2 (Master Plan §4). Key design decisions already made:
- §4.1: Add `UpstreamStream::Boxed(Box<dyn AsyncRead + AsyncWrite + Send + Unpin>)` with `AsyncRead`/`AsyncWrite`/`Debug` impls.
- §4.2: Add `ProfileType::External` (API-only, confirmed by user per §8.3 — not exposed in CLI TOML; with clear validation error if a declared external dialer is missing when building `RoutingState`), and `ProfileRegistry::with_external(name, Arc<dyn UpstreamDialer>)` (plus builder support on `RoutingState`).
- §4.3: Define `pub trait RouteResolver: Send + Sync + fmt::Debug` with `fn resolve(&self, req: &RouteRequest) -> Result<RouteDecision, AccessDenied>` (or returning the resolved decision + dialer handle needed by handlers), implemented by `RoutingState` / `SharedRouting`, and update HTTP/SOCKS handlers to accept `Arc<dyn RouteResolver>` (preserving existing `from_routing` convenience constructors).
- §4.6: Define a typed `DialError` enum (`Blocked`, `Filtered`, `NotConnected`, `AuthRejected`, `Timeout`, `Other`) wrapped inside `io::Error` at the `UpstreamDialer` boundary, with helper extractors so HTTP (`502`/`504`/`403`) and SOCKS reply-code mapping use it consistently.

## Scope
- In: `src/upstream/**`, `src/routing.rs`, `src/router/**`, `src/http/**`, `src/socks/**`, `src/server.rs`, `tests/**`
- Out: `src/mitm/**`, `src/logging.rs`, `../rusty-conduit/**`, `../rusty-conduit-ssh/**`

## Sub-tasks
1. [done] Implement §4.1: add `UpstreamStream::Boxed(Box<dyn AsyncRead + AsyncWrite + Send + Unpin>)` (and a convenience constructor `UpstreamStream::boxed(...)`), implement `Debug`, `AsyncRead`, `AsyncWrite`, and add a unit test relaying bytes through a `tokio::io::duplex` boxed stream — done when: `cargo test --lib upstream` passes including the boxed stream test
2. [done] Implement §4.6: add typed `DialError` (`Blocked`, `Filtered`, `NotConnected`, `AuthRejected`, `Timeout`) in `src/upstream/mod.rs` carried as an inner `io::Error` source (preserving `RouteBlocked` compatibility), add `dial_error(&io::Error)` helper, wire HTTP and SOCKS error-to-status/reply mapping to use it, and add unit tests for each variant's HTTP/SOCKS mapping — done when: `cargo test` passes including typed dial error tests
3. [done] Implement §4.2: add `ProfileType::External` and `ProfileRegistry::with_external(name, Arc<dyn UpstreamDialer>)` (plus `RoutingState` builder method to supply external dialers); validate that rules/chains referencing an external profile succeed only when declared and fail with a clear error if the external dialer was not supplied; add unit tests for missing, supplied, and failover-chained external dialers — done when: `cargo test` passes including external dialer tests
4. [done] Implement §4.3: define `RouteResolver` trait in `src/routing.rs`, implement it for `RoutingState` and `SharedRouting`, update `HttpProxy`, `SocksProxy`, and `EngineHandlers` to dispatch through `Arc<dyn RouteResolver>` while keeping existing `from_routing` constructors working, and add a test with a custom mock `RouteResolver` — done when: `cargo test` passes including the custom `RouteResolver` test
5. [done] Run full checks in `rusty-conduit-engine` and `../rusty-conduit-switch` to confirm zero regressions — done when: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && cargo clippy --all-targets -- -D warnings && cargo test)` exits 0

## Verify
```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && cargo clippy --all-targets -- -D warnings && cargo test)
```

## Escalate when
- `RouteResolver` signature conflicts with hot-reload (`SharedRouting` / `ArcSwap`) or `EngineHandlers` in `rusty-conduit-switch` in a way that breaks existing public API without a clear adapter → stop, set Status: needs-human — reason: describe the signature conflict
- Changing `DialError` mapping changes an existing HTTP status code or SOCKS reply code in `rusty-conduit-switch`'s integration tests → stop, set Status: needs-human — reason: list the test and expected vs actual code

## Log
- (append-only, one line per event)
- sub-task 1: added UpstreamStream::Boxed(Box<dyn BoxedIo>) (BoxedIo = blanket AsyncRead+AsyncWrite+Send+Unpin trait, since a multi-trait dyn is not valid Rust), UpstreamStream::boxed(), Debug/AsyncRead/AsyncWrite arms, duplex relay test
- sub-task 2: added DialError {Blocked, Filtered, NotConnected{kind}, AuthRejected, Timeout, Other{kind}} + into_io (Blocked carried as RouteBlocked, Filtered as AccessDenied for compatibility) + dial_error(&io::Error) classifier; socks5 auth failures and HTTP 407 now carry AuthRejected (same kind/message); HTTP dial_error_status and SOCKS Failure::from_dial_error route through it with unchanged codes; per-variant HTTP/SOCKS tests
- sub-task 3: added ProfileType::External (#[serde(skip)], API-only), DialerOptions::with_external / external registry, ProfileRegistry::with_external, RoutingState::with_external; dialer_for_profile errors clearly when an external profile has no dialer; tests for missing, supplied, and failover-chained external dialers
- sub-task 4: added `RouteResolver` trait (resolve(&RouteRequest) -> Result<RouteDecision, AccessDenied>, state() -> Arc<RoutingState> for dialing, pin() -> Arc<dyn RouteResolver> for hot-reload pinning) implemented for RoutingState, ArcSwap<RoutingState>, and SharedRouting; HttpProxy/SocksProxy hold Arc<dyn RouteResolver> (new(SharedRouting) kept, plus with_resolver), EngineHandlers::from_resolver added (from_routing kept); HTTP connections, SOCKS requests, and MITM per-request routing pin the resolver, so the existing snapshot semantics are unchanged; tests/route_resolver.rs covers a custom mock resolver end to end
- sub-task 5: full Verify run, exit 0; switch unchanged (path dependency on the engine), 126 switch tests pass

## Evidence
- (append-only: command → observed result)
- sub-task 1: `cargo test --lib upstream` → 20 passed incl. upstream::tests::boxed_stream_relays_bytes
- sub-task 2: `cargo test` → 144 passed (lib), incl. dial_error_classifies_and_round_trips, typed_dial_errors_map_to_http_status, typed_dial_errors_map_to_socks_replies
- sub-task 3: `cargo test` → 149 passed (lib), incl. external_profile_without_dialer_is_a_clear_error, external_profile_with_supplied_dialer_connects, external_dialers_fail_over_in_chains, routing_state_requires_declared_external_dialers, external_profile_type_is_api_only
- sub-task 4: `cargo test --test route_resolver` → 2 passed (custom_route_resolver_drives_http_and_socks_engines, routing_state_and_shared_routing_are_route_resolvers)
- sub-task 5 / Verify: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && cargo clippy --all-targets -- -D warnings && cargo test)` → EXIT=0; engine 149 lib + 2 integration tests pass; PURE_RUST_CRYPTO_OK; switch 54+12+8+8+3+17+10+14 = 126 tests pass, clippy clean
