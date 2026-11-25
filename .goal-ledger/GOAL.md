# GOAL — Build rusty-conduit-engine (E0–E2)

## Goal
- Goal ID: 20261125-engine-e0-e2
- Outcome: Bootstrap `rusty-conduit-engine`, extract the shared proxy/routing engine from `rusty-conduit-switch` with zero behavior change, and implement all v0.1.0 extension points and MITM CA loader fixes.
- Done when: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `python3 scripts/check-pure-rust-crypto.py` pass in `rusty-conduit-engine`, and `cargo test` plus `cargo clippy --all-targets -- -D warnings` pass in `../rusty-conduit-switch` on `feat/one-engine` with byte-identical `test-route` output.
- Goal status: completed
- Goal status meaning: drafting | approved | executing | blocked-on-human | in-review | awaiting-acceptance | completed | abandoned
- Last completed phase: phase-0006

## Execution
- Execution mode: delegated
- Planner tier: Gemini 4 Neon (Medium)
- Executor tier: herdr-subagent (claude opus medium; opencode default while waiting for claude limit reset)
- Full verification: cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && python3 scripts/check-pure-rust-crypto.py && (cd ../rusty-conduit-switch && cargo clippy --all-targets -- -D warnings && cargo test)

## Git
- Repository: yes
- Strategy: isolated-branch
- Starting branch: main
- Work branch: goal/20261125-engine-e0-e2
- Baseline commit: 2ee21b0bb62dba3720b594019deb08be649722dc
- Starting upstream at start: none
- Work upstream at start: none

## Phases
- [done] phase-0001 — E0: Bootstrap crate, CI, pure-Rust crypto check, and CLI work branches
- [done] phase-0002 — E1a: Extract engine modules and routing types from rusty-conduit-switch
- [done] phase-0003 — E1b: Port rusty-conduit-switch (feat/one-engine) onto rusty-conduit-engine
- [done] phase-0004 — E2a: Add boxed streams, external dialers, RouteResolver trait, and typed dial errors
- [done] phase-0005 — E2b: Add MITM CA from bytes, CA loader fixes, and observability hooks
- [done] phase-0006 — E2c: Add routing feature gate, divergence ledger doc, and v0.1.0 readiness checks

## Handoff
- Current position: completed
- Next role: planner
- Next action: none
- Last verified evidence: Gate D Full verification passed (engine 162 tests default + 150 --no-default-features, clippy -D warnings clean both, PURE_RUST_CRYPTO_OK; switch 127 tests + clippy clean on feat/one-engine after Claude review refinement 619863b; validate_goal_ledger.py 0 errors)
- Blockers: none

## Review
- Reviewed by: Gemini 4 Neon (Medium) (planner)
- Verification result: passed (exit 0; engine 162 tests default + 150 --no-default-features, clippy -D warnings clean both, PURE_RUST_CRYPTO_OK; switch 126 tests + clippy clean on feat/one-engine)
- Diff reviewed: 2ee21b0bb62dba3720b594019deb08be649722dc..014111a (46 files, 18787 insertions; all non-verbatim-copied diffs, switch test diffs, and scope verified in full)
- Findings: none (zero scope drift, all 6 phases verified, all sibling repos clean; optional squash preconditions satisfied)

## Log
- created ledger with 6 phases
- user approved Gates A-C, isolated branch goal/20261125-engine-e0-e2, starting executor opencode, and API-only external profile type (§8.3)
- phase-0001 done by executor (claude): crate skeleton, CI, crypto check, feat/one-engine branches; Verify passed
- phase-0002 done by executor (claude): engine modules extracted with RoutingInput/RoutingError, MitmOptions, ServerOptions, RoutingState in crate::routing; Verify passed
- phase-0003 done by executor (opencode): switch ported onto engine crate (re-exports + From<RoutingError> + engine constructors); fmt/clippy/test (126) + byte-identical test-route vs main pass; Verify passed; switch commit 1e317d4
- phase-0004 done by executor (claude): Boxed upstream streams, typed DialError + dial_error() mapping (HTTP/SOCKS codes unchanged), API-only ProfileType::External + with_external builders, RouteResolver trait (resolve/state/pin) used by HttpProxy/SocksProxy/EngineHandlers/MITM; Verify passed
- phase-0005 done by executor (claude): CaMaterial/MitmState::from_pem_bytes, explicit P-256/P-384 EC params rewritten (else CaError::ExplicitEcParameters), PBES2 HMAC-SHA1 PRF -> CaError::Pbes2Sha1Prf with corrected hint (no Cargo.toml change), AccessRecord sink/channel on AccessLogger, RoutingCounters (active/bytes_in/bytes_out/hits) on RoutingState wired into HTTP/CONNECT/MITM/SOCKS; Verify passed
- phase-0006 done by executor (claude): `routing` feature gates regex in router::condition (regex conditions error without it), regex-dependent tests gated, docs/DIVERGENCE-LEDGER.md created, CI checks/tests --no-default-features, README/lib docs updated; Verify passed; no tag/push
- Gate D planner review passed clean; set status to awaiting-acceptance
- post-Gate-D Claude review of phase-0003 (../rusty-conduit-switch 1e317d4) applied D1-D3 and C1-C6 in commit 619863b (127 tests pass, byte-identical CLI output)
- user accepted goal; squashed 20 commits on goal/20261125-engine-e0-e2 into one commit, marked completed, and merged into main
