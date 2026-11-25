# phase-0003 — E1b: Port rusty-conduit-switch (feat/one-engine) onto rusty-conduit-engine

- Status: done
- Owner: executor
- Depends on: phase-0002
- Goal: Switch `../rusty-conduit-switch` on branch `feat/one-engine` to depend on `../rusty-conduit-engine`, replace its copied engine modules with `pub use rusty_conduit_engine::...;` re-exports, and verify all tests, clippy, and byte-identical `test-route` output.
- Done when: In `../rusty-conduit-switch` on `feat/one-engine`, `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` exits 0, `test-route` output matches `main` byte-for-byte on fixtures, and `rusty-conduit-engine` still passes its own verification.
- Pattern to follow: `../rusty-conduit-switch/src/lib.rs:17-30`, `docs/MASTER-PLAN.md:94-98`

## Context
This completes Milestone E1. Work in `../rusty-conduit-switch` MUST happen on branch `feat/one-engine` (never on `main`). `rusty-conduit-switch` keeps its CLI/TOML layer (`src/config.rs`, `src/main.rs`, `src/lib.rs`, `src/reload.rs`, `src/import/`), re-exporting the extracted modules from `rusty_conduit_engine` so integration tests in `../rusty-conduit-switch/tests/` compile and pass unchanged. Capture `test-route` output on `main` (or before edits) to verify byte-identical output after porting.

Concrete API mappings from `phase-0002`:
- `rusty_conduit_engine::routing::Inbound` implements `FromStr` and `Inbound::ALL` (no `clap::ValueEnum` derive); in `src/config.rs`, keep `--inbound` working in `clap` via `value_parser` or a thin CLI-side wrapper converting to `Inbound`.
- Add helper methods on `Config` to produce `RoutingInput`, `MitmOptions`, and `ServerOptions`.
- Map `RoutingError` into `ConfigError` (`From<RoutingError>` or transparent variant) preserving exact error strings.
- `MitmError::Passphrase` wraps `mitm::PassphraseError`; `UpstreamError::Routing` replaces `UpstreamError::Config`; `Router::from_input` / `ProfileRegistry::from_input` / `RoutingState::from_input` / `MitmState::from_options` / `Server::from_options` take the new engine input structs.

## Scope
- In: `../rusty-conduit-switch/Cargo.toml`, `../rusty-conduit-switch/Cargo.lock`, `../rusty-conduit-switch/src/**` (on branch `feat/one-engine`), plus minor visibility/helper adjustments in `rusty-conduit-engine/src/**` if needed by `-switch`
- Out: `../rusty-conduit-switch/tests/**` (must remain unmodified except if an import path genuinely moved, per §2.2), `../rusty-conduit/**`, `../rusty-conduit-ssh/**`, any push/merge operations

## Sub-tasks
1. [done] In `../rusty-conduit-switch`, record baseline `cmd_test_route` / CLI `test-route` output on representative configs from `main` to a temp file, then checkout `feat/one-engine` — done when: `git -C ../rusty-conduit-switch branch --show-current` prints `feat/one-engine`
2. [done] Update `../rusty-conduit-switch/Cargo.toml` to add `rusty-conduit-engine = { path = "../rusty-conduit-engine" }` and keep only direct dependencies referenced by `rusty-conduit-switch`'s remaining `src/` and `tests/` files (`clap`, `notify`, `toml`, `serde`, `serde_json`, `tokio`, `tokio-util`, `humantime`, `thiserror`, `arc-swap`, `rustls`, `tokio-rustls`, `rcgen`, `hyper`, `hyper-util`, `http`, `http-body-util`, `bytes`, `socket2`, `time`) — done when: `../rusty-conduit-switch/Cargo.toml` declares the path dependency
3. [done] Delete the extracted module files from `../rusty-conduit-switch/src/` (`net.rs`, `filter.rs`, `mux.rs`, `logging.rs`, `server.rs`, `mitm/`, `upstream/`, `router/`, `http/`, `socks/`), re-export them in `../rusty-conduit-switch/src/lib.rs` (`pub use rusty_conduit_engine::{filter, http, logging, mitm, mux, net, router, routing, server, socks, upstream};`), and update `src/config.rs`, `src/lib.rs`, and `src/reload.rs` to re-export `routing::*` / `CaSource` and call the engine's constructors — done when: `cd ../rusty-conduit-switch && cargo check --all-targets` exits 0
4. [done] Run `../rusty-conduit-switch`'s full test suite, `cargo fmt --check`, and `cargo clippy --all-targets -- -D warnings`, and verify `test-route` output is byte-identical to the baseline captured in sub-task 1 — done when: all commands exit 0 and `diff` of `test-route` output is empty
5. [done] Commit the `feat/one-engine` changes in `../rusty-conduit-switch` with a clear commit message (e.g., `refactor: extract proxy engine to rusty-conduit-engine`), and verify `git -C ../rusty-conduit-switch status --porcelain` is clean — done when: `test -z "$(git -C ../rusty-conduit-switch status --porcelain)"`

## Verify
```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && test "$(git branch --show-current)" = "feat/one-engine" && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
```

## Escalate when
- Any integration test in `../rusty-conduit-switch/tests/` fails or requires modifying test assertions (not just trivial import paths) → stop, set Status: needs-human — reason: report the failing test and diff
- `test-route` output differs from `main` → stop, set Status: needs-human — reason: show the exact diff

## Log
- phase-0003 started; sub-task 1 done: baseline test-route captured on main to /tmp/rcs_baseline_main.txt (9 targets, http+socks), checked out feat/one-engine
- sub-task 3 done: extracted engine modules deleted from switch src/, re-exported from `rusty_conduit_engine`; `src/config.rs` re-exports routing model + `CaSource`, adds `From<RoutingError> for ConfigError`, helper `routing_input()/mitm_options()/server_options()`, delegates routing-model validation to engine; `src/lib.rs`, `src/reload.rs`, `src/import/mod.rs` call engine constructors; dead `canonical_ip`/`host_targets_listener` removed; `cargo check --all-targets` exits 0
- sub-task 4 done: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (126 pass: 54 lib + 12 airswitch + 8 hot_reload + 8 mitm + 3 mux + 17 routing + 10 socks + 14 upstream) all exit 0; `test-route` byte-identical to clean `main` on master_plan_example (9 targets)

## Evidence
- `git -C ../rusty-conduit-switch branch --show-current` → `feat/one-engine`
- sub-task 2: Cargo.toml now declares `rusty-conduit-engine = { path = "../rusty-conduit-engine" }`; pruned http, socket2, rustls-platform-verifier, dashmap (used only by deleted engine modules); kept base64, regex, p12-keystore, x509-parser (referenced by remaining src/import + tests)
- sub-task 3 analysis (engine API confirmed):
  - `rusty_conduit_engine::routing` has `RoutingInput { profiles, routing, filter, mitm_enabled, listeners, keepalive, connect_timeout }`, `RoutingError` (7 variants, Display strings identical to switch `ConfigError` routing variants), `ProfileConfig::resolve_password -> Result<_, RoutingError>`, `Router::from_input(&RoutingInput)`, `RoutingState::from_input(&RoutingInput, DialerOptions) -> RoutingSetupError`, `RoutingState`/`SharedRouting`/`RuntimeConfig`/`SharedRuntime` type aliases.
  - Engine `RoutingState` == switch `RuntimeConfig` (byte-identical struct); engine `RoutingSetupError` == switch `RoutingSetupError`. `RoutingState::new/with_forward_proxy/with_connect_timeout/with_generation/shared/first_upstream` all present; `RuntimeConfig::new` never called by switch.
  - Engine `Inbound` has NO `ValueEnum` (no clap dep) but implements `FromStr` (Err `ParseInboundError(pub String): Error+Send+Sync`) + `Display`; switch must re-export it and adapt clap via `value_parser = Inbound::from_str` (keep `default_value_t = Inbound::Http`).
  - `MitmOptions { ca: Option<CaSource>, passphrase, passphrase_file, cert_ttl, cert_cache_max }`; `MitmState::from_options(&MitmOptions)` matches switch original flow. `ServerOptions { bind, http_port, socks_port, max_connections, keepalive }`; `Server::from_options(&ServerOptions, handlers, logger)`.
  - `CaSource` (engine mitm) == switch `CaSource`. `router` re-exports `BypassList`, `RuleMatcher` (airswitch.rs keeps working). `import/mod.rs:374` `Router::from_config` -> `Router::from_input(&cfg.routing_input())`.
  - `credential_resolution_from_each_source` (src unit test) must expect `RoutingError` not `ConfigError` (return type genuinely moved).
- sub-task 3: `cargo check --all-targets` exits 0 after fixing `Inbound::from_str`/`Router::from_input`/`MitmState::build(&cfg.mitm_options(),...)` call sites and removing dead `canonical_ip`/`host_targets_listener` (engine `RoutingInput::validate()` covers self-loop) + removing unused `Ipv6Addr` import; `cargo fmt` + `cargo clippy --all-targets -- -D warnings` clean (fixed empty line after doc comment at config.rs:537).
- sub-task 4 byte-identical: rebuilt clean `main` (stashed switch edits) and `feat/one-engine` (edits) both against `master_plan_example` for 9 targets (http+socks); `diff` empty → porting preserves routing behavior. Original `/tmp/rcs_baseline_main.txt` was captured on a config with a separate `routing.socks` table (`default=bastion`) and no `ssh-over-bastion`/`socks-only-override` rule, so it is not reproducible from the repo; the 6 http targets that share config match it exactly, and the definitive main-vs-feat diff is byte-identical. Engine still passes its own verification: fmt/clippy/test (140) + `scripts/check-pure-rust-crypto.py` (PURE_RUST_CRYPTO_OK).
- sub-task 5 done: `git add -A && git commit` on `feat/one-engine` → `1e317d4 refactor: depend on extracted rusty-conduit-engine crate` (trailer `Goal-ID: 20261125-engine-e0-e2`, parent `69b0c41`); `git status --porcelain` clean.
