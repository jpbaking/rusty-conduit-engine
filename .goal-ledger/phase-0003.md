# phase-0003 — cli-support: fdlimit, preflight, shutdown_signal, app_version!

- Status: done
- Owner: executor
- Depends on: phase-0002
- Goal: Move the three CLIs' byte-identical `fdlimit.rs`, `preflight.rs`, and `shutdown_signal`, plus an `app_version!` macro, into `rusty_conduit_engine::cli_support`.
- Done when: every command in `## Verify` exits 0, and the moved unit tests run under `--features cli-support`.
- Pattern to follow: `../rusty-conduit/src/fdlimit.rs` and `../rusty-conduit/src/preflight.rs` (copy them; the ssh and switch copies are byte-identical), `../rusty-conduit/src/lib.rs:241-276` (`shutdown_signal`), `../rusty-conduit/src/config.rs:49-59` (`app_version`)

## Context
These files are identical in all three CLIs (verified by md5). Copy, do not rewrite: only `use` paths change (`crate::logging::Logger` stays valid inside the engine; `rusty_conduit_engine::mitm::...` becomes `crate::mitm::...`; `crate::fdlimit` becomes `crate::cli_support::fdlimit`).
`app_version` must become a macro: `option_env!("CONDUIT_RELEASE_VERSION")` and `env!("CARGO_PKG_VERSION")` must expand in the consumer crate, so a function in the engine would report the engine's own version. The planner already enabled `rlimit`, `libc`, and tokio `signal` under `cli-support` in phase-0002; do not touch `Cargo.toml`.

## Scope
- In: `src/cli_support/mod.rs`, `src/cli_support/fdlimit.rs` (new), `src/cli_support/preflight.rs` (new)
- Out: `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, every other `src/` file, the consumer repos (read-only sources)

## Sub-tasks
1. [done] Copy `../rusty-conduit/src/fdlimit.rs` to `src/cli_support/fdlimit.rs` unchanged except `use` paths; add `pub mod fdlimit;` to `src/cli_support/mod.rs` — done when: `cargo test --features cli-support --locked cli_support::fdlimit` passes.
2. [done] Copy `../rusty-conduit/src/preflight.rs` to `src/cli_support/preflight.rs` unchanged except `use` paths (`crate::mitm::{MitmOptions, MitmState}`, `crate::cli_support::fdlimit`); add `pub mod preflight;` — done when: `cargo test --features cli-support --locked cli_support::preflight` passes.
3. [done] Copy `shutdown_signal` (with its doc comment) from `../rusty-conduit/src/lib.rs:241-276` into `src/cli_support/mod.rs` as `pub async fn shutdown_signal(logger: &crate::logging::Logger)` — done when: `cargo clippy --all-targets --features cli-support --locked -- -D warnings` passes.
4. [done] In `src/cli_support/mod.rs` add, with the doc comment from `../rusty-conduit/src/config.rs:49-53` plus one line saying it expands in the calling crate: `#[macro_export] macro_rules! app_version { () => { match option_env!("CONDUIT_RELEASE_VERSION") { Some(version) if !version.is_empty() => version, _ => concat!(env!("CARGO_PKG_VERSION"), "-dev") } }; }` (formatted normally), and a unit test asserting `app_version!()` equals `concat!(env!("CARGO_PKG_VERSION"), "-dev")` when `CONDUIT_RELEASE_VERSION` is unset at build time (guard the assertion with `if option_env!("CONDUIT_RELEASE_VERSION").is_none()`) — done when: `cargo test --features cli-support --locked app_version` passes.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked
cargo test --all-features --locked
cargo test --no-default-features --features cli-support --locked
git diff --exit-code -- Cargo.toml Cargo.lock src/lib.rs src/routing.rs src/server.rs src/mitm
```

## Escalate when
- A copied file needs a change beyond `use` paths to compile → stop, set Status: needs-human — reason: paste the compiler error.
- A dependency is missing under `cli-support` → stop, set Status: needs-human — reason: name it (do not edit `Cargo.toml`).

## Log
- (append-only, one line per event)
- sub-task 1: copied `../rusty-conduit/src/fdlimit.rs` to `src/cli_support/fdlimit.rs` and exported `pub mod fdlimit;` in `src/cli_support/mod.rs`
- sub-task 2: copied `../rusty-conduit/src/preflight.rs` to `src/cli_support/preflight.rs` with updated `use` paths and exported `pub mod preflight;` in `src/cli_support/mod.rs`
- sub-task 3: copied `shutdown_signal` with its doc comment from `../rusty-conduit/src/lib.rs:241-276` into `src/cli_support/mod.rs`
- sub-task 4: added `#[macro_export] macro_rules! app_version!` with doc comment and unit test in `src/cli_support/mod.rs`
- verify: ran all `## Verify` commands and confirmed clean exit 0

- planner accepted phase-0003: fdlimit/preflight diff against ../rusty-conduit only in use lines, shutdown_signal identical, Verify re-run (225 / 234 all-features / 222 no-default+cli-support)

## Evidence
- (append-only: command → observed result)
- `cargo test --features cli-support --locked cli_support::fdlimit` → ok (4 passed, 0 failed)
- `cargo test --features cli-support --locked cli_support::preflight` → ok (4 passed, 0 failed)
- `cargo clippy --all-targets --features cli-support --locked -- -D warnings` → ok (0 warnings)
- `cargo test --features cli-support --locked app_version` → ok (1 passed, 0 failed)
- `cargo fmt --check` → exit 0
- `cargo clippy --all-targets --locked -- -D warnings` → exit 0 (0 warnings)
- `cargo clippy --all-targets --all-features --locked -- -D warnings` → exit 0 (0 warnings)
- `cargo test --locked` → ok (207 unit + 18 integration = 225 passed, 0 failed)
- `cargo test --all-features --locked` → ok (216 unit + 18 integration = 234 passed, 0 failed)
- `cargo test --no-default-features --features cli-support --locked` → ok (205 unit + 17 integration = 222 passed, 0 failed)
- `git diff --exit-code -- Cargo.toml Cargo.lock src/lib.rs src/routing.rs src/server.rs src/mitm` → exit 0
