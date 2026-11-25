# phase-0005 — E2b: Add MITM CA from bytes, CA loader fixes, and observability hooks

- Status: done
- Owner: executor
- Depends on: phase-0004
- Goal: Implement Master Plan §4.4 (`MitmState::from_pem_bytes`), §4.5 (structured `AccessRecord` sink and per-profile/per-rule atomic counters), and the two MITM CA loader fixes from `../rusty-conduit-ssh-gui/docs/TODO.md`, with comprehensive tests.
- Done when: All features and CA fixes are tested and passing, `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py` passes, and `../rusty-conduit-switch` tests still pass.
- Pattern to follow: `src/mitm/mod.rs`, `src/mitm/ca.rs`, `src/logging.rs`, `../rusty-conduit-ssh-gui/docs/TODO.md:1-19`

## Context
This is the second half of Milestone E2 (Master Plan §4.4, §4.5, and §6 E2 CA loader fixes):
- §4.4: `MitmState::from_pem_bytes(cert_chain_pem: &[u8], key_pem: &[u8], opts: &MitmOptions)` (plus `CaMaterial::from_pem_bytes` supporting optional passphrase) so embedders like the GUI never write vaulted CA material to temp files.
- CA loader fixes (from `../rusty-conduit-ssh-gui/docs/TODO.md`):
  1. EC keys with explicit curve parameters: either rewrite known P-256 / P-384 explicit parameters to named-curve OID or return a clear, dedicated `CaError` variant explaining explicit curve parameters are unsupported and how to re-encode with `-pkeyopt ec_param_enc:named_curve` (including in PKCS#12 bundles so it does not misreport "private key does not match CA certificate").
  2. Encrypted PKCS#8 using PBES2 with HMAC-SHA1 PRF (`1.2.840.113549.2.7`): either support the SHA-1 PRF cleanly with pure-Rust crates or fix the error message and hint (no longer suggesting LibreSSL's broken `-v2 aes-256-cbc`; suggest PKCS#12 or `-v1 PBE-SHA1-3DES` / OpenSSL 3 `-v2 aes-256-cbc -v2prf hmacWithSHA256`) — zero C crypto crates allowed!
- §4.5: Structured `AccessRecord` sink (callback or channel on `AccessLogger`) emitting records alongside formatted text/JSON lines; and per-profile / per-rule atomic counters (`active`, `bytes_in`, `bytes_out`, `hits`) readable from `RoutingState` and updated by proxy handlers / relay.

## Scope
- In: `src/mitm/**`, `src/logging.rs`, `src/routing.rs`, `src/http/**`, `src/socks/**`, `src/server.rs`, `tests/**`
- Out: `../rusty-conduit/**`, `../rusty-conduit-ssh/**`, adding any C crypto dependency

## Sub-tasks
1. [done] Implement §4.4: add `CaMaterial::from_pem_bytes` and `MitmState::from_pem_bytes` (plus custom upstream TLS variant if helpful) without touching the filesystem, and add unit tests loading valid, encrypted, and expired PEM byte slices — done when: `cargo test --lib mitm` passes including in-memory PEM tests
2. [done] Fix the two MITM CA loader issues from `../rusty-conduit-ssh-gui/docs/TODO.md` in `src/mitm/ca.rs` (explicit EC curve parameters in PEM & PKCS#12; PBES2 HMAC-SHA1 PRF error/hint or pure-Rust support) and add unit tests covering both cases — done when: `cargo test --lib mitm::ca && python3 scripts/check-pure-rust-crypto.py` exits 0
3. [done] Implement §4.5a: add a structured `AccessRecord` type and optional sink (e.g., `AccessLogger::with_sink(...)` taking `Arc<dyn Fn(&AccessRecord) + Send + Sync>` or `mpsc` sender) emitted alongside existing text/JSON log lines without changing formatted log output, plus unit tests — done when: `cargo test --lib logging` passes including structured sink tests
4. [done] Implement §4.5b: add per-profile and per-rule atomic counters (`active`, `bytes_in`, `bytes_out`, `hits`) readable from `RoutingState` (e.g., snapshot structs), wire connection handlers and relay byte counts to update them accurately (including decrementing `active` on drop/completion), and add integration tests verifying `hits`, `active`, `bytes_in`, and `bytes_out` — done when: `cargo test` passes including counter tests
5. [done] Run full checks in `rusty-conduit-engine` and `../rusty-conduit-switch` to confirm zero regressions — done when: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && cargo clippy --all-targets -- -D warnings && cargo test)` exits 0

## Verify
```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && cargo clippy --all-targets -- -D warnings && cargo test)
```

## Escalate when
- Fixing PBES2 HMAC-SHA1 would require pulling in a banned C crypto crate or unvetted dependency → stop, set Status: needs-human — reason: report which option (pure-Rust crate vs improved error/hint) you need approved
- Wiring byte counters changes existing access-log formatting or breaks `rusty-conduit-switch` tests → stop, set Status: needs-human — reason: report the exact failure

## Log
- (append-only, one line per event)
- begin commit 205b727; generated LibreSSL 3.3.6 fixtures (explicit-param P-256 PKCS#8/SEC1/PKCS#12, PBES2 HMAC-SHA1-PRF key) in scratchpad and embedded them in src/mitm/ca.rs tests
- st1: added CaMaterial::from_pem_bytes (parse + validate, optional passphrase) and MitmState::from_pem_bytes / from_pem_bytes_with_upstream (shared from_material builder; no CA file IO); unit tests for valid, chain, encrypted (ok/missing/wrong pass), expired, mismatched, and leaf signing
- st2: explicit EC params: PKCS#8, SEC1 and PKCS#12 keys whose explicit parameters equal P-256/P-384 are rewritten to the named-curve OID; others -> new CaError::ExplicitEcParameters (also in PKCS#12 instead of KeyMismatch). PBES2 HMAC-SHA1 PRF: chose the improved-error option (real support needs pkcs5 `sha1-insecure` feature in Cargo.toml, which is outside Scope In) -> new CaError::Pbes2Sha1Prf detected by parsing PBKDF2 params (explicit or default PRF) plus error-string fallback; LegacyEncryptedPem hint no longer suggests bare `-v2 aes-256-cbc`
- st3: added logging::AccessRecord enum, AccessRecordSink (Arc<dyn Fn(&AccessRecord)+Send+Sync>), AccessLogger::with_sink / with_channel / has_sink; http_enabled/socks_enabled also true with a sink; formatted lines unchanged
- st4: added routing::{TrafficCounters, TrafficSnapshot, RoutingCounters, TrafficTracker}; RoutingState.counters (+ with_counters, counters, profile_traffic, rule_traffic); wired HTTP forward, CONNECT tunnel, MITM (per decrypted request) via AccessGuard::track and SOCKS via Session::track; bytes added on completion, active decremented on drop; integration tests tests/traffic_counters.rs and tests/mitm_counters.rs
- st5: full Verify block passed

## Evidence
- (append-only: command → observed result)
- st1: `cargo test --lib mitm` → test result: ok. 35 passed; 0 failed (incl. mitm::tests::builds_state_from_in_memory_pem_bytes, in_memory_pem_bytes_honour_passphrase_and_validity, ca::tests::from_pem_bytes_loads_and_validates_in_memory_input)
- st2: `cargo test --lib mitm::ca && python3 scripts/check-pure-rust-crypto.py` → test result: ok. 24 passed; 0 failed; PURE_RUST_CRYPTO_OK (exit 0) (incl. accepts_libressl_explicit_p256_parameters_in_pem_and_pkcs12, accepts_explicit_p384_parameters, rejects_unknown_explicit_parameters_with_dedicated_error, reports_pbes2_hmac_sha1_prf_with_working_hint)
- st3: `cargo test --lib logging` → test result: ok. 17 passed; 0 failed (incl. structured_sink_receives_records_alongside_unchanged_lines, sink_only_logger_is_enabled_and_writes_nothing)
- st4: `cargo test --test traffic_counters` → ok. 1 passed; `cargo test --test mitm_counters` → ok. 1 passed; `cargo test` → lib 158 passed + mitm_counters 1 + route_resolver 2 + traffic_counters 1, 0 failed
- st5/Verify: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && cargo clippy --all-targets -- -D warnings && cargo test)` → exit=0; 15 test suites, 288 passed, 0 FAILED (engine 162, switch 126); PURE_RUST_CRYPTO_OK; clippy clean in both; switch on feat/one-engine with clean worktree
