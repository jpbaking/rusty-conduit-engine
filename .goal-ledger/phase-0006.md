# phase-0006 — import: move the AirSwitch importer into the engine

- Status: done
- Owner: executor
- Depends on: phase-0005
- Goal: Copy rusty-conduit-switch's AirSwitch / ZeroOmega / SwitchyOmega `.bak` converter and its commented-TOML renderer into `rusty_conduit_engine::import`, behind the `import` feature, with behavior unchanged.
- Done when: every command in `## Verify` exits 0, and all of the importer's own unit tests pass inside the engine.
- Pattern to follow: `../rusty-conduit-switch/src/import/airswitch.rs` (copy whole) and `../rusty-conduit-switch/src/import/mod.rs` (copy everything except the CLI subcommand)

## Context
M-02: the switch importer uses only engine `routing` / `router` types (re-exported in switch as `crate::config::*`), so it moves with path changes only. Rename `crate::config::` to `crate::routing::` and keep `crate::router::{BypassList, RuleMatcher}`. `run_import` (the `import-airswitch` subcommand: file I/O, `create_new`, exit codes) and `EXIT_FAILED` stay in switch — do not copy them or the tests that call them. Behavior changes (B-08) are phase-0007's; this phase must not change any output.
Escalation answered (planner, 2026-11-28): rustfmt sorts `use crate::router::...` before `use crate::routing::...`, so that one moved line is an allowed fidelity difference; the Verify filter now allows it.
The test `rendered_toml_parses_and_validates` uses switch's `Config` / `FileConfig`; inside the engine, parse the rendered text with the dev-dependency `toml` into a test-local `#[derive(Deserialize)] struct File { #[serde(default)] profiles: BTreeMap<String, ProfileConfig>, routing: RoutingConfig }` and build the router with `crate::router::Router::from_input(&RoutingInput { profiles: file.profiles, routing: file.routing, ..RoutingInput::default() })`.

## Scope
- In: `src/import/mod.rs`, `src/import/airswitch.rs` (new)
- Out: `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `src/routing.rs`, `src/router/`, every other `src/` file, the consumer repos (read-only sources)

## Sub-tasks
1. [done] Copy `../rusty-conduit-switch/src/import/airswitch.rs` to `src/import/airswitch.rs`; change its `use crate::config::{...}` to `use crate::routing::{...}` with the same names — done when: `cargo build --features import --locked` succeeds.
2. [done] Replace the planner's `src/import/mod.rs` body (keep the planner's module doc, adjusting it to describe `convert` and `render_toml`) with `../rusty-conduit-switch/src/import/mod.rs` minus `run_import`, `EXIT_FAILED`, the imports only they use, and the tests `run_import_refuses_overwrite_without_force` and `run_import_reports_errors` (and the `tmp` helper if only they use it); `use crate::config::{...}` becomes `use crate::routing::{...}` — done when: `cargo clippy --all-targets --features import --locked -- -D warnings` passes.
3. [done] Rewrite `rendered_toml_parses_and_validates` as described in Context — done when: `cargo test --features import --locked import::` passes with every remaining importer test.
4. [done] Copy-fidelity check: `diff ../rusty-conduit-switch/src/import/airswitch.rs src/import/airswitch.rs | grep '^[<>]' | grep -v 'crate::config::\|crate::routing::'` prints nothing (only the `use` path lines differ; wrap-around lines of that one `use` statement are allowed and must be listed in Evidence) — done when: the command prints nothing or only those `use`-statement lines, recorded in Evidence.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --all-features --locked
cargo test --features import --locked import::
test -z "$(diff ../rusty-conduit-switch/src/import/airswitch.rs src/import/airswitch.rs | grep '^[<>]' | grep -v 'crate::config\|crate::routing\|crate::router::{BypassList, RuleMatcher}\|^[<>]     [A-Za-z_, ]*$\|^[<>] };')"
git diff --exit-code -- Cargo.toml Cargo.lock src/lib.rs src/routing.rs src/router src/cli_support src/test_support.rs
```

## Escalate when
- The copied code uses a switch-only type (not a re-export of the engine) → stop, set Status: needs-human — reason: name the type and line.
- A copied test fails inside the engine → stop, set Status: needs-human — reason: name the test and paste the failure (do not change expected output).

## Log
- (append-only, one line per event)
- sub-task 1: copied `../rusty-conduit-switch/src/import/airswitch.rs` to `src/import/airswitch.rs` and changed `use crate::config::{` to `use crate::routing::{`
- sub-task 2: replaced `src/import/mod.rs` from switch's `src/import/mod.rs` without `run_import`/`EXIT_FAILED`/CLI-only imports and tests, keeping adjusted module doc and updating `crate::config::` to `crate::routing::`
- sub-task 3: rewrote `rendered_toml_parses_and_validates` in `src/import/mod.rs` using `toml::from_str` and `Router::from_input`
- sub-task 4 / Verify: ran `cargo fmt` because `cargo fmt --check` failed on `use crate::routing` before `use crate::router`; after `cargo fmt`, `cargo fmt --check`, clippy, and all tests pass, but `diff` emits `< / > use crate::router::{BypassList, RuleMatcher};`, failing `## Verify` line 31 — escalating to planner

- planner answered the escalation (rustfmt import order), widened the Verify filter, reviewed the full diff of both files against switch (only use lines, the removed `run_import` / `EXIT_FAILED` / their I/O imports and two tests, and the rewritten `rendered_toml_parses_and_validates`; switch's `cfg.validate()` has no engine equivalent and `Router::from_input` still validates the routing), re-ran ## Verify, and closed the phase

## Evidence
- (append-only: command → observed result)
- `cargo build --features import --locked` → exit 0 (`Finished dev profile ...`)
- `cargo clippy --all-targets --features import --locked -- -D warnings` → exit 0 (0 warnings)
- `cargo test --features import --locked import::` → exit 0 (18 passed; 0 failed)
- `cargo fmt --check` (before `cargo fmt`) → exit 1: rustfmt requires `use crate::router::{BypassList, RuleMatcher};` before `use crate::routing::{...};` in `src/import/airswitch.rs:28-34`
- `cargo fmt && cargo fmt --check` → exit 0
- `cargo clippy --all-targets --all-features --locked -- -D warnings` → exit 0
- `cargo clippy --all-targets --no-default-features --locked -- -D warnings` → exit 0
- `cargo test --all-features --locked` → exit 0 (243 unit + 5 error_pages + 4 mitm_counters + 6 mitm_h2_upstream_reconnect + 2 route_resolver + 1 traffic_counters passed; 0 failed)
- `diff ../rusty-conduit-switch/src/import/airswitch.rs src/import/airswitch.rs | grep '^[<>]' | grep -v 'crate::config::\|crate::routing::'` → printed `> use crate::router::{BypassList, RuleMatcher};` and `< use crate::router::{BypassList, RuleMatcher};`
- `test -z "$(diff ../rusty-conduit-switch/src/import/airswitch.rs src/import/airswitch.rs | grep '^[<>]' | grep -v 'crate::config\|crate::routing\|^[<>]     [A-Za-z_, ]*$\|^[<>] };')"` → exit 1 (failed on the two `use crate::router::{BypassList, RuleMatcher};` lines)
- planner: fmt/clippy (all-features, no-default) exit 0; `cargo test --all-features --locked` → 261 passed; `cargo test --features import --locked import::` → 18 passed; widened fidelity check → empty; out-of-scope git diff → exit 0
