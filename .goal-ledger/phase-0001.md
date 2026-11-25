# phase-0001 — E0: Bootstrap crate, CI, pure-Rust crypto check, and CLI work branches

- Status: done
- Owner: executor
- Depends on: none
- Goal: Create the `rusty-conduit-engine` library crate skeleton, BSD-3-Clause license, README, CI workflow, pure-Rust crypto check script, and `feat/one-engine` branches in the three sibling CLI repos.
- Done when: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py` exits 0 in `rusty-conduit-engine`, and `feat/one-engine` exists on `main`'s commit in `../rusty-conduit`, `../rusty-conduit-ssh`, and `../rusty-conduit-switch`.
- Pattern to follow: `../rusty-conduit-switch/Cargo.toml:1-67`, `../rusty-conduit-switch/LICENSE:1-29`, `../rusty-conduit-switch/.github/workflows/ci.yml:1-55`, `../rusty-conduit-ssh-gui/scripts/check-pure-rust-crypto.py:1-40`

## Context
This repository currently contains only `docs/MASTER-PLAN.md`. Milestone E0 establishes a building, CI-ready library-only Rust crate (`rusty-conduit-engine`, lib name `rusty_conduit_engine`, version `0.1.0`, edition `2021`, BSD-3-Clause) with pure-Rust TLS/crypto (`ring` only; no OpenSSL / `aws-lc`). It also creates the `feat/one-engine` branch from `main` in each of the three sibling CLI repositories without modifying any files in those CLI repos yet. Do not include CLI-only dependencies (`clap`, `notify`, `toml`) in `Cargo.toml`.

## Scope
- In: `Cargo.toml`, `Cargo.lock`, `.gitignore`, `LICENSE`, `README.md`, `src/lib.rs`, `scripts/check-pure-rust-crypto.py`, `.github/workflows/ci.yml`, plus creating local git branch `feat/one-engine` in `../rusty-conduit`, `../rusty-conduit-ssh`, and `../rusty-conduit-switch`
- Out: `docs/MASTER-PLAN.md`, any source files in `../rusty-conduit*`, any push/remote operations

## Sub-tasks
1. [done] Create `.gitignore` (ignoring `/target` and `.cargo/config.toml`), copy `LICENSE` from `../rusty-conduit-switch/LICENSE`, and copy `scripts/check-pure-rust-crypto.py` from `../rusty-conduit-ssh-gui/scripts/check-pure-rust-crypto.py` (chmod +x) — done when: `test -f .gitignore && cmp -s LICENSE ../rusty-conduit-switch/LICENSE && cmp -s scripts/check-pure-rust-crypto.py ../rusty-conduit-ssh-gui/scripts/check-pure-rust-crypto.py`
2. [done] Create library-only `Cargo.toml` (`name = "rusty-conduit-engine"`, `version = "0.1.0"`, `edition = "2021"`, `license = "BSD-3-Clause"`, `[lib] name = "rusty_conduit_engine"`, `[features] default = ["routing"]`, `routing = ["dep:regex"]`) with the shared engine dependencies from `../rusty-conduit-switch/Cargo.toml` (tokio without `signal`/`io-std`, tokio-util, bytes, hyper, hyper-util, http, http-body-util, rustls with `ring`/`std`/`tls12`/`logging`, tokio-rustls with `ring`/`tls12`/`logging`, rustls-platform-verifier, rcgen with `crypto`/`pem`/`ring`/`x509-parser`, p12-keystore, x509-parser, socket2, dashmap, arc-swap, optional regex, base64, serde with `derive`, serde_json, humantime, time with `formatting`/`macros`/`local-offset`, thiserror; dev-dependency `socket2` with `all`) and create a minimal `src/lib.rs` with crate doc comment and one smoke unit test — done when: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check` exits 0
3. [done] Generate `Cargo.lock` via `cargo test` and verify `python3 scripts/check-pure-rust-crypto.py` prints `PURE_RUST_CRYPTO_OK` — done when: `test -f Cargo.lock && python3 scripts/check-pure-rust-crypto.py` exits 0
4. [done] Create `README.md` describing `rusty-conduit-engine` (purpose, consumers, pure-Rust crypto policy, BSD-3-Clause license, link to `docs/MASTER-PLAN.md`) and `.github/workflows/ci.yml` modeled on `../rusty-conduit-switch/.github/workflows/ci.yml` (without the `docker` job) running `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `python3 scripts/check-pure-rust-crypto.py`, and `cargo test --locked` on Linux, macOS, and Windows — done when: `test -f README.md && test -f .github/workflows/ci.yml && grep -q "check-pure-rust-crypto.py" .github/workflows/ci.yml`
5. [done] Create local branch `feat/one-engine` from `main` (without switching away from `main` if already clean, or verify clean working tree) in `../rusty-conduit`, `../rusty-conduit-ssh`, and `../rusty-conduit-switch` using `git -C ../<repo> branch feat/one-engine main` (only if the branch does not already exist) — done when: `for r in rusty-conduit rusty-conduit-ssh rusty-conduit-switch; do git -C "../$r" rev-parse --verify feat/one-engine >/dev/null || exit 1; done`

## Verify
```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && for r in rusty-conduit rusty-conduit-ssh rusty-conduit-switch; do git -C "../$r" rev-parse --verify feat/one-engine >/dev/null || exit 1; done
```

## Escalate when
- `../rusty-conduit`, `../rusty-conduit-ssh`, or `../rusty-conduit-switch` has a dirty working tree or an existing `feat/one-engine` branch pointing to a different commit than `main` → stop, set Status: needs-human — reason: describe which repo and its git status
- `python3 scripts/check-pure-rust-crypto.py` reports a banned crypto crate (`openssl-sys` or `aws-lc-sys`) → stop, set Status: needs-human — reason: paste the `cargo tree --invert` output
- Network is unavailable to resolve crates.io dependencies for `Cargo.lock` → stop, set Status: needs-human — reason: cargo failed to fetch dependencies

## Log
- (append-only, one line per event)
- began phase-0001 (commit acfd5f7)
- st1: created .gitignore (/target, .cargo/config.toml), copied LICENSE and scripts/check-pure-rust-crypto.py (chmod +x)
- st2: created library-only Cargo.toml (engine deps, tokio without signal/io-std, regex optional behind `routing`) and src/lib.rs with crate doc + smoke test
- st3: Cargo.lock generated by cargo test; pure-Rust crypto check passes
- st4: created README.md and .github/workflows/ci.yml (lint+crypto job on Linux, test matrix Linux/macOS/Windows, no docker job)
- st5: created local branch feat/one-engine at main in rusty-conduit, rusty-conduit-ssh, rusty-conduit-switch (all clean, all stayed on main)
- verify passed; note: cargo reports manifest-level "unused dependency" warnings (13) for engine deps not yet used by src; these are not rustc/clippy warnings and do not fail -D warnings

## Evidence
- (append-only: command → observed result)
- st1: `test -f .gitignore && cmp -s LICENSE ../rusty-conduit-switch/LICENSE && cmp -s scripts/check-pure-rust-crypto.py ../rusty-conduit-ssh-gui/scripts/check-pure-rust-crypto.py` → exit 0
- st2: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check` → exit 0; `test result: ok. 1 passed`
- st3: `test -f Cargo.lock && python3 scripts/check-pure-rust-crypto.py` → `PURE_RUST_CRYPTO_OK`, exit 0
- st4: `test -f README.md && test -f .github/workflows/ci.yml && grep -q "check-pure-rust-crypto.py" .github/workflows/ci.yml` → exit 0
- st5: pre-check `git status --short` empty in all three repos, no existing feat/one-engine; after `git branch feat/one-engine main`: rusty-conduit db2c82a8 = main, rusty-conduit-ssh 88bbe0c9 = main, rusty-conduit-switch 69b0c417 = main; done-when loop → exit 0
- Verify block (full) → exit 0; `test result: ok. 1 passed`; `PURE_RUST_CRYPTO_OK`
