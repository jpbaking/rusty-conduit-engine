# phase-0005 — test-support: shared test fixtures

- Status: done
- Owner: executor
- Depends on: phase-0004
- Goal: Publish the test fixtures rusty-conduit and rusty-conduit-ssh both copy (logger capture, echo server, HTTP origin, test CA, TLS client, connect-through helper) as `rusty_conduit_engine::test_support` behind the `test-support` feature.
- Done when: every command in `## Verify` exits 0, and a smoke test exercises each published helper.
- Pattern to follow: `../rusty-conduit/src/test_support.rs:1-270` (copy it; it is the reference copy, ssh's differs only in `TestCa.cert_der` type and doc wording)

## Context
`../rusty-conduit/src/test_support.rs` is a `#[cfg(test)]` module used by that crate's unit tests. Every engine item it uses is already public (`http::mitm_tunnel::client_config_with_roots`, `mitm::ca::CaMaterial`, `mitm::crypto_provider`, `mitm::signer::LeafSigner`, `logging::*`), and every crate it uses (`bytes`, `http-body-util`, `hyper`, `hyper-util`, `rcgen`, `rustls`, `time`, `tokio`, `tokio-rustls`) is already a normal engine dependency, so the planner made `test-support = []`. Inside the engine its `crate::` paths stay valid. Make each item `pub` (it was `pub(crate)` or private-to-module), keep behavior and names unchanged, and add a `//!` module doc saying it is for consumers' tests and is not covered by semver stability.

## Scope
- In: `src/test_support.rs`
- Out: `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, every other `src/` file, the consumer repos (read-only sources)

## Sub-tasks
1. [done] Replace the planner's empty `src/test_support.rs` with the contents of `../rusty-conduit/src/test_support.rs`, keeping the planner's module doc at the top; change every item's visibility to `pub` (struct fields that tests read, e.g. `TestCa` / `Origin` fields, too) — done when: `cargo build --features test-support --locked` succeeds.
2. [done] Add `#[cfg(test)] mod tests` at the end of `src/test_support.rs` with `#[tokio::test] async fn fixtures_work()`: start `spawn_echo` on `127.0.0.1:0` and check `echo_roundtrip` returns its input; build `test_ca()` and assert its cert DER is non-empty; start `spawn_origin` and fetch one response with a plain TCP request; use `quiet_logger()` once — done when: `cargo test --features test-support --locked test_support` passes.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-features --locked
cargo test --features test-support --locked test_support
git diff --exit-code -- Cargo.toml Cargo.lock src/lib.rs src/routing.rs src/cli_support
```

## Escalate when
- The copied file needs a dependency that is not already in `Cargo.toml` → stop, set Status: needs-human — reason: name the crate (do not edit `Cargo.toml`).
- A helper's signature does not match the smoke-test description above → write the smoke test against the real signature and note it in `## Log`; anything beyond that → stop, set Status: needs-human — reason: describe it.

## Log
- (append-only, one line per event)
- sub-task 1: copied `../rusty-conduit/src/test_support.rs` into `src/test_support.rs` preserving the planner's module doc; all public fixtures and struct fields (`Buf`, `TestCa`, `Origin`) are `pub`.
- sub-task 2: added `#[cfg(test)] mod tests` with `fixtures_work()` exercising `quiet_logger`, `capture_access`, `spawn_echo`/`echo_roundtrip`, `test_ca`, and `spawn_origin` + `tls_client` (note: `spawn_origin` is an HTTP/1.1 TLS origin rather than plain TCP, so the smoke test connects over TLS via `tls_client` trusting `origin.ca_der`).
- verify: all 6 `## Verify` commands passed cleanly.

- planner accepted phase-0005: diff vs ../rusty-conduit/src/test_support.rs is only the module doc plus the smoke test; Verify re-run (243 all-features; test-support-only lib clippy clean)

## Evidence
- (append-only: command → observed result)
- `cargo build --features test-support --locked` → exit 0 (`Finished dev profile ... in 2.73s`)
- `cargo test --features test-support --locked test_support` → exit 0 (`test test_support::tests::fixtures_work ... ok`; 1 passed, 0 failed)
- `cargo fmt --check && cargo clippy --all-targets --all-features --locked -- -D warnings && cargo clippy --all-targets --locked -- -D warnings && cargo test --features test-support --locked test_support && cargo test --all-features --locked && git diff --exit-code -- Cargo.toml Cargo.lock src/lib.rs src/routing.rs src/cli_support` → exit 0 (fmt clean, both clippy checks clean, 225 unit + 18 integration = 243 all-features tests passed, out-of-scope diff empty)
