# phase-0006 — E2c: Add routing feature gate, divergence ledger doc, and v0.1.0 readiness checks

- Status: done
- Owner: executor
- Depends on: phase-0005
- Goal: Verify the default-on `routing` cargo feature gate works cleanly, create `docs/DIVERGENCE-LEDGER.md` (§6.2) recording E1/E2 decisions, and confirm the engine and `rusty-conduit-switch` are completely ready for the `v0.1.0` review gate.
- Done when: Both default features and `--no-default-features` build/check cleanly (or `routing` feature boundary is verified), `docs/DIVERGENCE-LEDGER.md` exists, and the full verification command passes with zero warnings.
- Pattern to follow: `docs/MASTER-PLAN.md:68,118-120`

## Context
Master Plan §3 notes that routing types are gated behind a default-on `routing` cargo feature so a consumer can opt out if binary size ever matters, and §6.2 requires a divergence ledger tracking per-module differences/fixes (such as the two MITM CA loader fixes in E2) so nothing is dropped silently before E3–E5. Do NOT create git tags (`v0.1.0`) or push anything in this phase; tagging happens only after Gate D review and explicit user approval.

## Scope
- In: `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `src/router/**`, `src/routing.rs`, `src/http/**`, `src/socks/**`, `src/server.rs`, `src/upstream/**`, `tests/**`, `docs/DIVERGENCE-LEDGER.md`, `README.md`, `.github/workflows/ci.yml`
- Out: `../rusty-conduit/**`, `../rusty-conduit-ssh/**`, `../rusty-conduit-switch/**`, git tags, git push

## Sub-tasks
1. [done] Audit and complete the default-on `routing` cargo feature gate in `Cargo.toml`, `src/`, and `tests/` (`required-features = ["routing"]` or `#![cfg(feature = "routing")]` on routing-specific integration tests) so `cargo check --no-default-features` and `cargo test --no-default-features` pass cleanly (gating `regex`-dependent rule matching behind `#[cfg(feature = "routing")]` while keeping core `net`, `filter`, `mux`, `logging`, `mitm`, `upstream`, `http`, `socks`, `server`, `RouteResolver` usable) — done when: `cargo check --no-default-features && cargo test --no-default-features && cargo test` exits 0
2. [done] Create `docs/DIVERGENCE-LEDGER.md` documenting the §6.2 divergence ledger structure and recording the E1 extraction baseline (`rusty-conduit-switch`) plus E2 fixes (EC explicit curve parameters, PBES2 HMAC-SHA1) with their `fix` / `keep` / `option` classifications — done when: `test -f docs/DIVERGENCE-LEDGER.md`
3. [done] Update `.github/workflows/ci.yml` to also check `cargo check --no-default-features --locked` and verify `README.md` matches the actual crate API and feature flags — done when: `grep -q "no-default-features" .github/workflows/ci.yml`
4. [done] Run the full verification suite across `rusty-conduit-engine` (default and `--no-default-features`) and `../rusty-conduit-switch` (`feat/one-engine`) — done when: all checks exit 0 with clean working trees

## Verify
```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo check --no-default-features && cargo test --no-default-features && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
```

## Escalate when
- Gating `routing` cleanly behind `--no-default-features` requires major refactoring of `http`/`socks`/`server` beyond `RouteResolver` → stop, set Status: needs-human — reason: describe which modules are tightly coupled to `router`

## Log
- (append-only, one line per event)
- began phase (commit c19bbdd)
- sub-task 1: `routing` feature already in Cargo.toml (regex optional) but `src/router/condition.rs` imported regex unconditionally; added cfg-gated import, an uninhabited `Regex` stand-in + local `escape` fallback, and a no-routing `build_regex` returning `ConditionError` naming the feature; gated 12 regex-dependent unit tests + `PROFILES` const with `#[cfg(feature = "routing")]`, `tests/traffic_counters.rs` with `#![cfg(feature = "routing")]` (host_wildcard fixture); added no-routing test `regex_conditions_require_routing_feature`. No http/socks/server refactor needed.
- sub-task 2: created docs/DIVERGENCE-LEDGER.md (decision legend, entry format, E1-01..03 switch extraction baseline from main 69b0c41 / feat 1e317d4, E2-01 EC explicit params fix, E2-02 PBES2 HMAC-SHA1 fix, E2-03..07 extension points keep, E2-08 routing feature option)
- sub-task 3: ci.yml adds `cargo check --no-default-features --locked`, no-default clippy (lint job) and `cargo test --no-default-features --locked` (test matrix); README Features/Modules/Development/Plan updated to match API (RouteResolver lives in `routing`); src/lib.rs doc notes the feature
- sub-task 4 + Verify: full suite exit 0, zero warnings; switch tree clean on feat/one-engine

## Evidence
- (append-only: command → observed result)
- `cargo check --no-default-features` (before) → error[E0432]: unresolved import `regex` at src/router/condition.rs:26
- `cargo check --no-default-features && cargo test --no-default-features && cargo test` → EXIT=0 (no-default: lib 147 passed; default: lib 158 passed, integration 1+2+1)
- `cargo clippy --all-targets --no-default-features -- -D warnings` → EXIT=0
- `test -f docs/DIVERGENCE-LEDGER.md` → exit 0
- `grep -q "no-default-features" .github/workflows/ci.yml` → EXIT=0
- Verify block (full command from ## Verify) → EXIT=0, `grep -c warning:` on log → 0, PURE_RUST_CRYPTO_OK, switch tests all ok (54/12/8/8/3/17/10/14 passed)
- `git -C ../rusty-conduit-switch status --short` → empty (branch feat/one-engine); Cargo.lock unchanged
