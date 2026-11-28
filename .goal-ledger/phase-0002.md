# phase-0002 — Contract amendment, opt-in features, and aligned default constants

- Status: done
- Owner: planner
- Depends on: phase-0001
- Goal: Amend the engine contract to allow CLI support and test fixtures behind opt-in features, add the `cli-support`, `test-support`, and `import` features with their dependencies and empty gated modules, extend CI to `--all-features`, and align the default connection and MITM cache constants with the CLIs.
- Done when: every command in `## Verify` exits 0; default features pull in no new dependency; `DOX.md` states the amended contract.
- Pattern to follow: `Cargo.toml:12-14` (the existing `routing = ["dep:regex"]` opt-in feature) and `src/lib.rs:18-28` (module list)

## Context
M-01: the engine contract (`DOX.md` Project Contract, "No CLI concerns") forbids signal handling and CLI-oriented dependencies; the roadmap explicitly asks to permit them strictly behind opt-in features. Decision: `cli-support = ["dep:rlimit", "dep:libc", "tokio/signal"]` (`rlimit` and `libc` as `[target.'cfg(unix)'.dependencies]`, optional); `test-support = []` (fixtures use only existing dependencies); `import = ["routing"]` (the importer validates regexes). Still no `clap`, `notify`, or non-dev `toml`.
Default constants: the CLIs use `DEFAULT_MAX_CONNECTIONS = 4096` and `DEFAULT_MITM_CERT_CACHE_MAX_ENTRIES = 10_240`; the engine has `server::DEFAULT_MAX_CONNECTIONS = 1024` and `mitm::DEFAULT_MITM_CERT_CACHE_MAX = 4096`. Align the engine to 4096 and 10 240 so Phase 3b consumers can use the engine constants; record it as `M-16` (`keep`) in `docs/DIVERGENCE-LEDGER.md`.
Planner-owned: contract text, feature design, and a behavior change.

## Scope
- In: `DOX.md`, `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `src/cli_support/mod.rs` (new), `src/test_support.rs` (new), `src/import/mod.rs` (new), `src/server.rs` and `src/mitm/mod.rs` (the two constants and tests that assert them), `docs/DIVERGENCE-LEDGER.md`, `.github/workflows/ci.yml`, `.github/workflows/release.yml` (verification steps only)
- Out: `src/routing.rs`, `src/router/`, `scripts/check-pure-rust-crypto.py` (consumers copy it unchanged)

## Sub-tasks
1. [done] `DOX.md` Project Contract: rewrite "No CLI concerns" to allow, only behind the opt-in `cli-support` feature, resource-limit raising (`rlimit`), path preflight (`libc`), shutdown-signal waiting (tokio `signal`), and shared config-file section types (serde only); still no `clap`, `notify`, non-dev `toml`, argument parsing, file watching, or SSH supervision. Add `test-support` (fixtures for consumers' tests) and `import` (`.bak` importer) to the feature bullets; add the `--all-features` checks to Verification — done when: `grep -c 'cli-support' DOX.md` prints at least `2`.
2. [done] `Cargo.toml` features and optional deps; `cargo update -p` nothing else (only the new optional entries may appear in `Cargo.lock`) — done when: `cargo tree -e normal --target all --depth 1 | grep -cE '(rlimit|libc) v'` prints `0` and the same with `--features cli-support` prints `2` (direct dependencies only: `libc` is already a transitive dependency of tokio and socket2).
3. [done] Empty gated modules with module docs: `#[cfg(feature = "cli-support")] pub mod cli_support;`, `#[cfg(feature = "test-support")] pub mod test_support;`, `#[cfg(feature = "import")] pub mod import;` — done when: `cargo check --all-features --locked` and `cargo check --no-default-features --locked` pass.
4. [done] Constants 4096 / 10 240, adjust any test asserting the old values, add `M-16` to the divergence ledger — done when: `cargo test --locked` passes and `grep -c '^| M-16' docs/DIVERGENCE-LEDGER.md` prints `1`.
5. [done] CI: add `cargo clippy --all-targets --all-features --locked -- -D warnings` and `cargo test --all-features --locked` to `.github/workflows/ci.yml` (lint and test jobs) and the same test step to `release.yml`'s verify job — done when: `grep -c 'all-features' .github/workflows/ci.yml` prints at least `2` and both files parse with `ruby -ryaml`.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
cargo test --all-features --locked
python3 scripts/check-pure-rust-crypto.py
test "$(cargo tree -e normal --target all --depth 1 | grep -cE '(rlimit|libc) v')" = 0
test "$(cargo tree -e normal --target all | grep -c 'rlimit v')" = 0
ruby -ryaml -e 'ARGV.each { |f| YAML.load_file(f) }' .github/workflows/ci.yml .github/workflows/release.yml
git diff --exit-code -- src/routing.rs src/router scripts
```

## Escalate when
- Adding the optional dependencies changes any other `Cargo.lock` entry → stop and record why before continuing.
- A consumer-visible test asserts the old constant values in a way that implies a contract → record it in the divergence entry.

## Log
- (append-only, one line per event)
- planner begins phase-0002 (planner-owned)
- planner corrected the dependency check: `libc` is already a transitive default dependency (tokio, socket2), so the check now looks at direct dependencies (`--depth 1`) and at `rlimit` anywhere

- sub-task 1: DOX contract — "No CLI concerns in default builds" plus three opt-in feature bullets; Verification gains the two --all-features checks; Feature Map gains "Opt-in features"
- sub-task 2: features cli-support / test-support / import; optional unix deps rlimit 0.12 and libc 0.2; Cargo.lock gained only rlimit, signal-hook-registry, errno (tokio `signal`) — no existing entry changed
- sub-task 3: gated module stubs in src/lib.rs with module docs
- sub-task 4: DEFAULT_MAX_CONNECTIONS 1024 → 4096, DEFAULT_MITM_CERT_CACHE_MAX 4096 → 10_240; no test asserted the literal values; M-16 (keep) added
- sub-task 5: ci.yml lint + test jobs and release.yml verify job run --all-features clippy/test
- note: cargo warns "unused dependency `rlimit`" until phase-0003 adds the code; not a clippy failure
- phase-0002 verified and closed

## Evidence
- (append-only: command → observed result)
- `cargo fmt --check` → exit 0; clippy default / --no-default-features / --all-features → exit 0
- `cargo test --locked` → 225 passed; `--no-default-features` → 213 passed; `--all-features` → 225 passed; PURE_RUST_CRYPTO_OK
- `cargo tree -e normal --target all --depth 1 | grep -cE '(rlimit|libc) v'` → 0; with `--features cli-support` → 2 (libc, rlimit); `rlimit` anywhere in the default tree → 0
- `grep -c 'cli-support' DOX.md` → 3; `grep -c '^| M-16' docs/DIVERGENCE-LEDGER.md` → 1; `grep -c 'all-features' .github/workflows/ci.yml` → 2; ruby YAML parse of both workflows → exit 0
