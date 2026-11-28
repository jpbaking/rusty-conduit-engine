# GOAL — Engine v0.7.0 API: Serialize routing config, cli-support, test-support, and import features

## Goal
- Goal ID: 20261128-engine-v070-api
- Outcome: rusty-conduit-engine gains `Serialize` on its routing config types and three opt-in features — `cli-support` (fdlimit, preflight, shutdown signal, `app_version!`, shared `[mitm]`/`[logging]` file sections), `test-support` (shared TLS/echo/origin test fixtures), and `import` (the AirSwitch / SwitchyOmega `.bak` importer moved from rusty-conduit-switch, with the B-08 fixes) — so the four consumers can delete their copies in roadmap Phase 3b after one `v0.7.0` bump.
- Done when: the Full verification command exits 0; with default features `cargo tree -e normal --target all --depth 1` lists neither `rlimit` nor `libc` as a direct engine dependency and `rlimit` appears nowhere (opt-in only); every phase `## Verify` passes; `docs/DIVERGENCE-LEDGER.md` has an entry for every behavior difference introduced (default constants, importer B-08 fixes); and `git diff <baseline>..HEAD` touches only paths under some phase's `Scope: In`.
- Goal status: completed
- Goal status meaning: drafting | approved | executing | blocked-on-human | in-review | awaiting-acceptance | completed | abandoned
- Last completed phase: phase-0007

## Execution
- Execution mode: delegated
- Planner tier: Claude Code (claude-opus-5-5)
- Executor tier: agy --model gemini-4-neon-medium via the herdr-subagent skill (fallback: claude --model opus --effort medium via herdr-subagent while agy is rate-limited)
- Full verification: cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo clippy --all-targets --no-default-features --locked -- -D warnings && cargo clippy --all-targets --all-features --locked -- -D warnings && cargo test --locked && cargo test --no-default-features --locked && cargo test --all-features --locked && python3 scripts/check-pure-rust-crypto.py

## Git
- Repository: yes
- Strategy: isolated-branch
- Starting branch: main
- Work branch: goal/20261128-engine-v070-api
- Baseline commit: 39548ae9a86d50e2378481757f3fe0195e7f0f84
- Starting upstream at start: origin/main@39548ae9a86d50e2378481757f3fe0195e7f0f84
- Work upstream at start: none

## Phases
- [done] phase-0001 — Serialize on routing config types
- [done] phase-0002 — Contract amendment, opt-in features, and aligned default constants
- [done] phase-0003 — cli-support: fdlimit, preflight, shutdown_signal, app_version!
- [done] phase-0004 — cli-support: shared [mitm] and [logging] file sections
- [done] phase-0005 — test-support: shared test fixtures
- [done] phase-0006 — import: move the AirSwitch importer into the engine
- [done] phase-0007 — import: B-08 fixes, divergence ledger, and docs

## Handoff
- Current position: completed; accepted by the user 2026-11-28; squashed into one commit and merged to main
- Next role: planner
- Next action: none (v0.7.0 tag deferred by the user until the remaining optimizations land)
- Last verified evidence: Gate D → Full verification exit 0 (225 / 213 / 265 tests, PURE_RUST_CRYPTO_OK)
- Blockers: none

## Review
- Reviewed by: planner, Claude Code (claude-opus-5-5)
- Verification result: pass — the Full verification command exits 0; `cargo tree -e normal --target all --depth 1` lists no `rlimit` / `libc` by default and `rlimit` appears nowhere in the default tree
- Diff reviewed: 39548ae..5c7e94a — read in full for all planner- and executor-written code (routing Serialize, Cargo/CI/DOX/ledger, cli_support/mod.rs and file.rs, B-08 changes); the three verbatim copies (fdlimit.rs, preflight.rs, test_support.rs, import/airswitch.rs, import/mod.rs) were checked by `diff` against their sources (only `use` lines, module docs, removed CLI-only code, and new tests differ); 17 non-ledger paths, all within phase Scope: In; 18 Goal-ID commits, no merges or foreign commits
- Findings: none open. Fixed during execution: a doc line wrap in phase-0001, the dependency-check command (libc is transitive by default) in phase-0002, the fidelity filter (rustfmt import order) in phase-0006. Intentional: the importer's rendered header still names rusty-conduit-switch (its output is a switch config). Deferred by plan: shared clap flag struct, ReverseProxy extraction, #[non_exhaustive], P-09 (v0.7.1), switch B-07/P-11

## Log
- created ledger with 7 phases; replaces the completed ledger 20261128-engine-optimization-phase2 (kept in Git history)
- source plan: rusty-conduit-series/docs/OPTIMIZATION.md roadmap Phase 3 engine items M-01 and M-02 (incl. B-08); P-09 is a separate later goal and tag (v0.7.1) — user decision "API first, P-09 later" (2026-11-28)
- planner decisions (scope): no `clap` or `toml` in the engine — `cli-support` needs only `rlimit`, `libc`, and tokio `signal`; the shared clap flag struct and `ReverseProxy` extraction are deferred (per-CLI env prefixes, ssh help-text variants, switch deprecated aliases, and consumer-config coupling need their own design); ssh's duplicate SOCKS5 client needs no engine change (`upstream::socks5::{handshake, encode_connect_request}` are already public) and is Phase 3b consumer work; `#[non_exhaustive]` on engine option structs is deferred (it would break every consumer's struct literals at the bump)
- planner decisions (design): `app_version!` is a `macro_rules!` because `option_env!`/`env!("CARGO_PKG_VERSION")` must expand in the consumer crate; shared file sections are named-table structs (`[mitm]`, `[logging]`) because `deny_unknown_fields` cannot be combined with `serde(flatten)`; the importer keeps hand-rendered TOML (no `toml` dependency)
- known Phase 3b concern (not this goal): conduit-core's `RoutingRule` has no `deny_unknown_fields` while the engine's `RuleConfig` does; adopting the engine types in the GUI needs a check against persisted data
- user approved the goal as drafted (Gate A), the isolated branch (Gate B), and execution (Gate C) on 2026-11-28 in one reply
- Gate B: branch goal/20261128-engine-v070-api created from main at baseline 39548ae9a86d50e2378481757f3fe0195e7f0f84
- delegated phase-0001 to agy (herdr-subagent)
- phase-0001 completed: derived `Serialize` with `skip_serializing_if` on all routing config types in `src/routing.rs`, documented password serialization, added round-trip and default-omission tests; all verify checks passed
- planner accepted phase-0001 (Verify re-run, diff in scope, doc line wrapped); began phase-0002
- phase-0002 done (planner): contract amendment, opt-in features, constants (M-16), CI --all-features
- delegated phase-0003 to agy (herdr-subagent)
- phase-0003 completed: copied `fdlimit.rs` and `preflight.rs`, added `shutdown_signal` and `app_version!` macro with unit test in `src/cli_support/`; all verify checks passed
- planner accepted phase-0003; began phase-0004
- phase-0004 done (planner): shared [mitm]/[logging] sections and merge helpers; two divergence candidates logged for phase-0007
- delegated phase-0005 to agy (herdr-subagent)
- phase-0005 completed: published shared test fixtures in `src/test_support.rs` behind `test-support` with `fixtures_work` smoke test; all verify checks passed
- planner accepted phase-0005; delegated phase-0006 to agy (herdr-subagent)
- phase-0006 blocked (needs-human): `cargo fmt --check` and `## Verify` line 31 copy-fidelity diff conflict on `src/import/airswitch.rs` (`use crate::router::{BypassList, RuleMatcher};` reordered before `use crate::routing::{...}` by rustfmt)
- planner answered phase-0006 escalation (rustfmt import order vs the fidelity filter); phase-0006 closed by the planner
- phase-0007 done (planner): B-08 fixes, M-17/M-18, docs; status in-review
- Gate D passed; status awaiting-acceptance
- user accepted 2026-11-28 and chose squash; all squash checks passed (clean tree, on the work branch, every commit carries the Goal-ID, no merges, nothing reachable from origin, origin/main still at the baseline); squashed onto the baseline, merged to main, pushed; no v0.7.0 tag yet (user: finish all optimizations first)
