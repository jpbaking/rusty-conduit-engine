# phase-0001 — Serialize on routing config types

- Status: done
- Owner: executor
- Depends on: none
- Goal: Derive `Serialize` on every routing config type in `src/routing.rs`, omitting default-valued fields, so consumers can write the engine types back out instead of keeping their own copies.
- Done when: every command in `## Verify` exits 0, including the new round-trip tests.
- Pattern to follow: `../rusty-conduit-gui/crates/conduit-core/src/v2_model.rs:420-470` (the GUI's copy of `RoutingRule`, which already uses `#[serde(default, skip_serializing_if = "Option::is_none")]` on each optional field)

## Context
M-02: the GUI keeps a duplicate routing AST only because the engine types are `Deserialize`-only. Output must be minimal TOML: absent `Option`s, `false` bools, and empty lists are not written, so a serialized config reads like a hand-written one and re-parses to an equal value.
`ProfileType::External` is `#[serde(skip)]` (API-only); leave that attribute as is — serializing an `External` profile is an error, which is correct.
`ProfileConfig` serializes `password` as stored. That is the caller's responsibility (the GUI never stores inline passwords); state it in the type's doc comment.
The crate's `toml` is a dev-dependency only; use it in tests, never in non-test code.

## Scope
- In: `src/routing.rs`
- Out: `Cargo.toml`, `Cargo.lock`, `src/router/`, every other `src/` file, `DOX.md`

## Sub-tasks
1. [done] In `src/routing.rs`, add `Serialize` next to `Deserialize` in the derives of `Inbound`, `OneOrMany`, `PortSpec`, `HostLevels`, `RuleConfig`, `RoutingTableConfig`, `RoutingConfig`, `ProfileType`, and `ProfileConfig`, and import it (`use serde::{Deserialize, Serialize};`) — done when: `cargo build --locked` and `cargo build --no-default-features --locked` succeed.
2. [done] Add skip attributes, keeping every existing `#[serde(...)]` attribute: on each `Option<_>` field of `HostLevels`, `RuleConfig` (all except `target`), `RoutingTableConfig.default`, `RoutingConfig.http` / `.socks`, and `ProfileConfig` (all `Option` fields) add `skip_serializing_if = "Option::is_none"`; on the `bool` fields `RuleConfig.match_all`, `.disabled`, `.bypass_mitm`, `RoutingTableConfig.inherit`, `RoutingConfig.resolve_dns`, and `ProfileConfig.tls_insecure_skip_verify` add `skip_serializing_if = "std::ops::Not::not"`; on the `Vec` fields `RoutingTableConfig.rules`, `RoutingConfig.rules`, and `ProfileConfig.bypass` add `skip_serializing_if = "Vec::is_empty"`. Where a field already has `#[serde(default)]`, write `#[serde(default, skip_serializing_if = "...")]` — done when: `cargo clippy --all-targets --locked -- -D warnings` passes.
3. [done] Add to `ProfileConfig`'s doc comment the sentence: `Serializing writes \`password\` as stored; callers that persist a config must not keep secrets inline.` — done when: `grep -c 'must not keep secrets inline' src/routing.rs` prints `1`.
4. [done] In the existing `#[cfg(test)] mod tests` of `src/routing.rs`, add test `serialize_round_trips_every_field`: a TOML string with `[profiles.corp]` (type `https`, host, port, username, password_env, bypass, tls_insecure_skip_verify = true), `[profiles.split]` (type `composite`, http_proxy, fallback_proxy), and a `[routing]` with `default = ["corp", "direct"]`, `resolve_dns = true`, one rule setting every `RuleConfig` field (use `port = [22, "8000-8080"]`, `host_levels = { min = 1, max = 3 }`, `inbound = "http"`, `match_all = true`, `bypass_mitm = true`), plus `[routing.http]` with `inherit = true`; parse it into a test-local `#[derive(Debug, PartialEq, Serialize, Deserialize)] struct File { #[serde(default)] profiles: BTreeMap<String, ProfileConfig>, routing: RoutingConfig }` with `toml::from_str`, serialize with `toml::to_string`, parse again, and assert both values are equal — done when: `cargo test --locked serialize_round_trips_every_field` passes.
5. [done] Add test `serialize_omits_defaults`: serialize a `RoutingConfig` holding one `RuleConfig` whose only set field is `target = "direct"` (all `Option`s `None`, bools `false`) and assert the output contains `target = "direct"` and none of `match_all`, `disabled`, `bypass_mitm`, `name`, `resolve_dns`, `http`, `socks` — done when: `cargo test --locked serialize_omits_defaults` passes.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
python3 scripts/check-pure-rust-crypto.py
git diff --exit-code -- Cargo.toml Cargo.lock src/router
```

## Escalate when
- A derive fails because a field type does not implement `Serialize` (other than `ProfileType::External`, which stays skipped) → stop, set Status: needs-human — reason: name the type and field.
- The round-trip test fails with unequal values → stop, set Status: needs-human — reason: paste the two values (do not change the test's TOML to make it pass).
- An existing test fails → stop, set Status: needs-human — reason: name the test and paste the failure.

## Log
- (append-only, one line per event)
- sub-task 1: imported `serde::Serialize` and derived `Serialize` on all 9 routing config types in `src/routing.rs`
- sub-task 2: added `skip_serializing_if` attributes on all `Option`, `bool`, and `Vec` fields of `HostLevels`, `RuleConfig`, `RoutingTableConfig`, `RoutingConfig`, and `ProfileConfig`
- sub-task 3: added password-serialization warning sentence to `ProfileConfig` doc comment
- sub-task 4: added `serialize_round_trips_every_field` unit test in `src/routing.rs`
- sub-task 5: added `serialize_omits_defaults` unit test in `src/routing.rs`
- verify: ran all `## Verify` commands; all exited 0

- planner accepted phase-0001: re-ran ## Verify (225 default / 213 no-default tests, PURE_RUST_CRYPTO_OK), read the diff (in scope); wrapped the new ProfileConfig doc line to 100 columns (wrapped so the sub-task 3 phrase stays on one line)

## Evidence
- (append-only: command → observed result)
- `cargo build --locked && cargo build --no-default-features --locked` → exit 0 (both builds succeeded)
- `cargo clippy --all-targets --locked -- -D warnings` → exit 0 (no warnings)
- `grep -c 'must not keep secrets inline' src/routing.rs` → `1`
- `cargo test --locked serialize_round_trips_every_field` → exit 0 (1 passed; 0 failed)
- `cargo test --locked serialize_omits_defaults` → exit 0 (1 passed; 0 failed)
- `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo clippy --all-targets --no-default-features --locked -- -D warnings && cargo test --locked && cargo test --no-default-features --locked && python3 scripts/check-pure-rust-crypto.py && git diff --exit-code -- Cargo.toml Cargo.lock src/router` → exit 0 (207+18 default tests passed, 196+17 no-default-features tests passed, `PURE_RUST_CRYPTO_OK`, out-of-scope files unchanged)
- planner: `grep -c 'must not keep secrets inline' src/routing.rs` → 1 after the wrap
