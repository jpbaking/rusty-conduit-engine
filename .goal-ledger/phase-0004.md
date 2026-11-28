# phase-0004 — cli-support: shared [mitm] and [logging] file sections

- Status: done
- Owner: planner
- Depends on: phase-0003
- Goal: Provide `cli_support::file` with the `[mitm]` and `[logging]` config-file section structs and the merge helpers the three CLIs duplicate, so Phase 3b can delete their copies.
- Done when: every command in `## Verify` exits 0, and unit tests cover each CLI's precedence rules (CLI > file > default) for every field, including switch's deprecated `mitm.cert_cache_max` key.
- Pattern to follow: `../rusty-conduit/src/config.rs:371-398` (`FileMitmConfig`, `FileLoggingConfig`), `../rusty-conduit/src/config.rs:554-663` (`merge`), `../rusty-conduit-switch/src/config.rs:594-622` (switch's variant with `cert_cache_max`)

## Context
M-01: the three CLIs repeat the `[mitm]` / `[logging]` structs and their merge logic (rusty-conduit and ssh identical; switch adds the deprecated `cert_cache_max` key with precedence CLI `_entries` > file `cert_cache_max_entries` > file `cert_cache_max` plus a NOTICE). `deny_unknown_fields` cannot be combined with `serde(flatten)`, so the shared types are named-table structs the CLIs embed as `mitm: Option<FileMitmConfig>` / `logging: Option<FileLoggingConfig>`; top-level keepalive and allow/block keys stay in each CLI. Design (field set, helper signatures, how deprecation notices surface) is decided while running this phase and recorded in `## Log`. serde only; no `toml` or `clap`.

## Scope
- In: `src/cli_support/mod.rs`, `src/cli_support/file.rs` (new), `DOX.md` (Feature Map line for cli-support)
- Out: `Cargo.toml`, `Cargo.lock`, `src/routing.rs`, `src/mitm/` (consume its public types only), the consumer repos

## Sub-tasks
1. [done] `FileMitmConfig` (incl. a deprecated `cert_cache_max: Option<usize>` alongside `cert_cache_max_entries`) and `FileLoggingConfig`, `Deserialize` + `deny_unknown_fields`, with `file_duration(key, value)` — done when: unit tests parse each CLI's documented `[mitm]` / `[logging]` example and reject an unknown key.
2. [done] Merge helpers producing the MITM settings (CA source, passphrase source, TTL, cache size with the deprecated-key precedence and notice, insecure/bypass host lists, insecure flag) and logging settings from CLI-side values plus the file section — done when: precedence tests pass for every field.
3. [done] DOX Feature Map entry for `cli-support` (Start: `src/cli_support/mod.rs`) — done when: `grep -c 'cli_support' DOX.md` prints at least `1`.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --all-features --locked
cargo test --no-default-features --features cli-support --locked
git diff --exit-code -- Cargo.toml Cargo.lock src/routing.rs src/mitm
```

## Escalate when
- The three CLIs' merge semantics cannot be expressed by one helper without changing one CLI's behavior → record the difference as a divergence-ledger candidate in `## Log` and keep each behavior selectable.

## Log
- (append-only, one line per event)
- planner begins phase-0004 (planner-owned)
- design: `cli_support::file` — `FileMitmConfig` (incl. deprecated `cert_cache_max`), `FileLoggingConfig`, `file_duration`, `FileValueError { key, message }` (Display identical to the CLIs' `ConfigError::InvalidFileValue`), `MitmCliValues` → `merge_mitm` → `MitmSettings { …, notices }`, `LoggingCliValues` → `merge_logging` → `LoggingSettings`; defaults are the engine's `DEFAULT_MITM_CERT_TTL` / `DEFAULT_MITM_CERT_CACHE_MAX` (= the CLIs' values after M-16) and INFO / text
- design: `cert_cache_max` precedence CLI `_entries` > file `_entries` > file `cert_cache_max`, and the notice (switch's exact text, `DEPRECATED_CERT_CACHE_MAX_NOTICE`) is emitted whenever the deprecated key is present, as switch does; rusty-conduit and ssh never had the key, so for them it is only a newly accepted spelling
- divergence candidate for phase-0007's ledger entry: the CLIs' `FileMitmConfig` derived `Debug` (printing `ca_passphrase`); the engine's `FileMitmConfig`, `MitmCliValues`, and `MitmSettings` redact it (engine contract)
- divergence candidate: rusty-conduit and ssh did not accept `mitm.cert_cache_max`; the shared section does (with switch's notice)
- phase-0004 verified and closed

## Evidence
- (append-only: command → observed result)
- `cargo test --all-features --locked cli_support::file` → 8 passed (sections parse / reject unknown keys, defaults, file fill-in, CLI wins every field, deprecated-key precedence + notice, error keys and text, logging precedence, Debug redaction)
- `cargo clippy --all-targets --all-features` and `--no-default-features` → exit 0; `cargo test --all-features --locked` → 242 passed; `--no-default-features --features cli-support` → 230 passed; `git diff --exit-code -- Cargo.toml Cargo.lock src/routing.rs src/mitm` → exit 0; `grep -c 'cli_support' DOX.md` → 3
