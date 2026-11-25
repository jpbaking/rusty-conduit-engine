# phase-0002 — E1a: Extract engine modules and routing types from rusty-conduit-switch

- Status: done
- Owner: executor
- Depends on: phase-0001
- Goal: Move the shared proxy, MITM, logging, upstream, router, routing, and server modules from `../rusty-conduit-switch/src/` into `rusty-conduit-engine/src/` with clean config/error boundaries so the engine compiles and passes all unit tests standalone.
- Done when: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py` exits 0 in `rusty-conduit-engine` with all extracted unit tests passing.
- Pattern to follow: `../rusty-conduit-switch/src/lib.rs:17-29`, `../rusty-conduit-switch/src/config.rs:57-680`

## Context
Milestone E1 extracts the shared engine out of `rusty-conduit-switch` without changing proxy behavior. In `rusty-conduit-switch`, engine modules import a handful of types from `crate::config` (`Inbound`, `ProfileConfig`, `ProfileType`, `PROFILE_DIRECT`, `PROFILE_BLOCK`, `RoutingConfig`, `RoutingTableConfig`, `RuleConfig`, `HostLevels`, `OneOrMany`, `Target`, `PortSpec`, `CaSource`, `DEFAULT_MITM_CERT_CACHE_MAX`, `Config`, `ConfigError`). In `rusty-conduit-engine`, CLI/TOML loading does NOT exist: routing/profile serde types live in `src/routing.rs`, MITM options (`CaSource`, `MitmOptions`) live in `src/mitm/mod.rs`, and server listener options (`ServerOptions` / `ListenPlan`) live in `src/server.rs`. Do not edit `../rusty-conduit-switch` in this phase; that happens in `phase-0003`.

## Scope
- In: `src/lib.rs`, `src/net.rs`, `src/filter.rs`, `src/mux.rs`, `src/logging.rs`, `src/routing.rs`, `src/mitm/**`, `src/upstream/**`, `src/router/**`, `src/http/**`, `src/socks/**`, `src/server.rs`, `Cargo.toml`, `Cargo.lock`
- Out: `../rusty-conduit-switch/**`, `../rusty-conduit/**`, `../rusty-conduit-ssh/**`, `docs/MASTER-PLAN.md`

## Sub-tasks
1. [done] Copy the self-contained modules (`net.rs`, `filter.rs`, `mux.rs`, `logging.rs`) from `../rusty-conduit-switch/src/` into `src/`, export them in `src/lib.rs`, and run `cargo test` — done when: `cargo test --lib` compiles and passes all tests in `net`, `filter`, `mux`, `logging`
2. [done] Create `src/routing.rs` containing the routing & profile serde types extracted from `../rusty-conduit-switch/src/config.rs` (`PROFILE_DIRECT`, `PROFILE_BLOCK`, `Inbound`, `OneOrMany`, `Target`, `PortSpec`, `HostLevels`, `RuleConfig`, `RoutingTableConfig`, `RoutingConfig`, `ProfileType`, `ProfileConfig`, `RoutingInput` / `RoutingError`), preserving exact serde attributes and validation behavior — done when: `cargo check --lib` succeeds with `pub mod routing;` in `src/lib.rs`
3. [done] Copy `src/mitm/` (`mod.rs`, `ca.rs`, `cache.rs`, `signer.rs`) from `../rusty-conduit-switch/src/mitm/`, move `CaSource`, `DEFAULT_MITM_CERT_TTL`, `MIN_MITM_CERT_TTL`, `DEFAULT_MITM_CERT_CACHE_MAX`, and `MitmOptions` into `src/mitm/mod.rs` (replacing `crate::config::Config` dependency with `MitmOptions`), and wire `pub mod mitm;` in `src/lib.rs` — done when: `cargo test --lib mitm` passes
4. [done] Copy `src/upstream/` and `src/router/` from `../rusty-conduit-switch/src/`, repointing `crate::config::*` imports to `crate::routing::*` (and replacing `Config`/`ConfigError` parameters in `Router` and `ProfileRegistry` with `RoutingInput`/`RoutingError` so no CLI `Config` is needed), and export them in `src/lib.rs` — done when: `cargo test --lib upstream && cargo test --lib router` passes
5. [done] Copy `src/http/`, `src/socks/`, and `src/server.rs` from `../rusty-conduit-switch/src/`, repointing `crate::config::Inbound` / `ProfileType` to `crate::routing::*`, replacing `Server::from_config(&Config, ...)` with `Server::from_options(&ServerOptions, ...)` (or `Server::bind` + `ServerOptions`), moving/re-exporting `RoutingState` and `SharedRouting` in `crate::routing` (with a re-export in `crate::server` for compatibility), and exporting all §3 modules in `src/lib.rs` — done when: `cargo test` passes all unit tests across all modules
6. [done] Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `python3 scripts/check-pure-rust-crypto.py` and fix any warnings or formatting issues — done when: all four commands exit 0

## Verify
```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py
```

## Escalate when
- Any extracted unit test in `router` or `upstream` depends on `CliInput` or `FileConfig` TOML parsing in a way that cannot be cleanly rewritten to construct `RoutingInput` / `ProfileConfig` directly without adding `toml` as a non-dev dependency → stop, set Status: needs-human — reason: describe which tests need `toml` (note: adding `toml` under `[dev-dependencies]` is allowed)
- A module requires an architectural change to `Config` or `HostFilter` not described above → stop, set Status: needs-human — reason: describe the exact coupling

## Log
- (append-only, one line per event)
- began phase (commit a31c623)
- ST1: copied net.rs, filter.rs, mux.rs, logging.rs verbatim; added tokio `io-std` feature (logging uses `tokio::io::stdout`)
- ST2: created src/routing.rs with PROFILE_DIRECT/BLOCK, Inbound, OneOrMany, Target, PortSpec, HostLevels, RuleConfig, RoutingTableConfig, RoutingConfig, ProfileType, ProfileConfig copied with identical serde attributes; added RoutingError (same messages as switch ConfigError variants), RoutingInput (profiles, routing, filter, mitm_enabled, listeners, keepalive, connect_timeout) with validate()/validate_profiles()/validate_targets()/validate_no_self_loop()/profiles_with_builtins(), host_targets_listener, DEFAULT_CONNECT_TIMEOUT. Inbound loses clap `ValueEnum` (no clap in engine); gains `FromStr` + `Inbound::ALL` instead
- ST3: copied mitm/; CaSource, DEFAULT_MITM_CERT_TTL, MIN_MITM_CERT_TTL, DEFAULT_MITM_CERT_CACHE_MAX, MitmOptions (+ resolve_passphrase, PassphraseError with the switch's message) now in mitm/mod.rs; MitmState::from_config(&Config) -> from_options(&MitmOptions), build() takes &MitmOptions. Note: mitm/mod.rs imports crate::http::mitm_tunnel, so `cargo test --lib mitm` could only compile once ST5's http/ was present; ST3-ST5 checks were run after all three copies
- ST4: copied upstream/ and router/; config imports -> crate::routing; Router::from_config/from_config_with_listeners -> from_input/from_input_with_listeners(&RoutingInput); validate_config/validate_with_listeners take &RoutingInput; configured_listeners removed (RoutingInput.listeners); ProfileRegistry::from_config -> from_input; UpstreamError::Config(ConfigError) -> Routing(RoutingError). Tests rebuilt via routing::test_support::input(toml) (toml added under [dev-dependencies] only; supports bind/http_port/socks_port/[filter]/[profiles]/[routing])
- ST5: copied http/, socks/, server.rs; Inbound/ProfileType imports -> crate::routing; reload::RuntimeConfig/SharedRuntime/RoutingSetupError moved into crate::routing as RoutingState/SharedRouting (from_input replaces from_config) with RuntimeConfig/SharedRuntime aliases; server re-exports them; ServerOptions (bind, ports, max_connections, keepalive, is_multiplexed, active_ports, listeners) + ListenPlan::from_options + Server::from_options replace Config; lib.rs exports all modules
- ST6: cargo fmt applied; clippy/test/crypto clean; fixed one redundant doc link I introduced and two "rusty-conduit-switch" doc mentions

## Evidence
- (append-only: command → observed result)
- ST1: `cargo test --lib` (net/filter/mux/logging only) → `test result: ok. 36 passed; 0 failed`
- ST2: `cargo build --lib` with `pub mod routing;` → `Finished dev profile` (no errors)
- ST3: `cargo test` → mitm tests included in `140 passed; 0 failed` (26 mitm tests, same count as switch src/mitm)
- ST4: `cargo test` → router (31) + upstream (14) tests pass, counts equal to switch's `#[test]`/`#[tokio::test]` counts; first run had 1 failure (`router::tests::global_filter_runs_first`: test_support rejected `[filter]`), fixed by adding `[filter]` to test_support
- ST5: `cargo test` → `test result: ok. 140 passed; 0 failed` (137 extracted tests, per-module counts equal to switch: net 7, filter 6, mux 7, logging 15, server 11, mitm 26, upstream 14, router 31, http 7, socks 13; plus 3 new: lib 1, routing 2)
- ST6: `cargo fmt --check` → exit 0; `cargo clippy --all-targets -- -D warnings` → `Finished`, no warnings; `python3 scripts/check-pure-rust-crypto.py` → `PURE_RUST_CRYPTO_OK`
- Verify: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py` → 140 passed, PURE_RUST_CRYPTO_OK, VERIFY_EXIT=0
