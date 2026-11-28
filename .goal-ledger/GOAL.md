# GOAL — Engine P-09: indexed routing and filter matching, lighter hot path, reload-safe counters

## Goal
- Goal ID: 20261128-engine-p09-router-index
- Outcome: `Router` and `HostFilter` match hosts through hash sets, suffix sets, and regex sets instead of one regex per rule, runs of host-only rules are looked up through an index that keeps first-match order, the per-request path stops re-normalizing hosts, cloning targets, querying the local clock, and looking counters up in maps, and a reload keeps counters only for profiles and rules that still exist (B-07 engine support) — with routing decisions unchanged.
- Done when: the Full verification command exits 0; the equivalence tests (indexed result == linear-scan result over generated corpora) pass; `cargo run --release --example route_bench` shows the 5000-rule host-only case at least 10× faster than the baseline recorded in phase-0001; every behavior difference has a `docs/DIVERGENCE-LEDGER.md` entry; and `git diff <baseline>..HEAD` touches only paths under some phase's `Scope: In`.
- Goal status: completed
- Goal status meaning: drafting | approved | executing | blocked-on-human | in-review | awaiting-acceptance | completed | abandoned
- Last completed phase: phase-0005

## Execution
- Execution mode: delegated
- Planner tier: Claude Code (claude-opus-5-5)
- Executor tier: agy --model gemini-4-neon-medium via the herdr-subagent skill (fallback: claude --model opus --effort medium via herdr-subagent while agy is rate-limited)
- Full verification: cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo clippy --all-targets --no-default-features --locked -- -D warnings && cargo clippy --all-targets --all-features --locked -- -D warnings && cargo test --locked && cargo test --no-default-features --locked && cargo test --all-features --locked && python3 scripts/check-pure-rust-crypto.py

## Git
- Repository: yes
- Strategy: isolated-branch
- Starting branch: main
- Work branch: goal/20261128-engine-p09-router-index
- Baseline commit: 55330cd6dbf4d2a9f0fbae0da713ae5b8d7700b3
- Starting upstream at start: origin/main@55330cd6dbf4d2a9f0fbae0da713ae5b8d7700b3
- Work upstream at start: none

## Phases
- [done] phase-0001 — Routing benchmark example and baseline
- [done] phase-0002 — Indexed conditions and destination filter
- [done] phase-0003 — First-match index across host-only rule runs
- [done] phase-0004 — Hot-path allocation, clock, and counter lookups
- [done] phase-0005 — Reload-safe counters, divergence entries, docs, and after-numbers

## Handoff
- Current position: completed; accepted by the user 2026-11-28; squashed into one commit and merged to main
- Next role: planner
- Next action: none (no tag until every optimization is done; next: roadmap Phase 3b with consumers pinned by rev)
- Last verified evidence: Gate D → Full verification exit 0 (721 tests, PURE_RUST_CRYPTO_OK); 5000-rule host ≈100×, wildcard ≈3900× faster
- Blockers: none

## Review
- Reviewed by: planner, Claude Code (claude-opus-5-5)
- Verification result: pass — Full verification exit 0 on all three feature sets; equivalence tests (wildcard fast paths vs full regex, combined vs separate regexes, compiled vs plain filter, indexed vs linear first match over 3 seeds) pass; benchmark meets the ≥10× done-when (5000 host ≈100×, wildcard ≈3900×; mixed and url not slower than baseline)
- Diff reviewed: 55330cd..e028b32, read in full (all code written by the planner except examples/route_bench.rs, reviewed at phase-0001 acceptance); 9 non-ledger paths, all within phase Scope: In (phase-0002 scope amended for the router's filter field); 11 Goal-ID commits, no merges; no non-test unwraps or debug output; no dependency or Cargo.lock change; no declared MSRV (CI uses stable), so `Option::is_none_or` is fine
- Findings: none open. Fixed during execution: the per-rule hash-set regression (NameSet small/large), the one-segment-per-rule overhead (linear ranges). Skipped with reason: pre-resolved counters (phase-0004). Deferred and recorded in docs/TODO.md: direct-dial filter scan in net.rs, PinnedRouting by value, URL / mixed rules still linear

## Log
- created ledger with 5 phases; replaces the completed ledger 20261128-engine-v070-api (kept in Git history)
- source plan: rusty-conduit-series/docs/OPTIMIZATION.md P-09, plus the engine half of B-07 (switch counter pruning / unnamed-rule keys)
- user decisions (2026-11-28): no engine tag until every optimization is done, so P-09 lands on the same untagged `main` as v0.7.0 (no separate v0.7.1); Phase 3b follows with consumers pinned by `rev`
- planner decisions: no new dependencies (no `aho-corasick`: `Keyword` keeps its substring scan; `regex::RegexSet` is already in `regex`); `PinnedRouting` by value is deferred — it changes the public `RouteResolver::pin` signature, which every embedder implements, for one allocation per request; P-11 needs no engine change (`RoutingState::new(router, registry)` already takes a precompiled `Router`) and is Phase 3b consumer work
- planner decisions: equivalence over raw speed — every index keeps a linear-scan reference path in tests, and any pattern the index cannot classify falls back to the existing regex
- user approved the goal (Gate A) on 2026-11-28; Gate B: branch goal/20261128-engine-p09-router-index created from main at baseline 55330cd6dbf4d2a9f0fbae0da713ae5b8d7700b3; Gate C: execution started (approval covered branch + start, as in the previous goal)
- delegated phase-0001 to agy (herdr-subagent)
- phase-0001 done: added `examples/route_bench.rs` and recorded two baseline runs in `.goal-ledger/phase-0001.md`
- planner accepted phase-0001 (baseline recorded); began phase-0002
- phase-0002 done (planner): indexed conditions and filter; scope amended for the router's filter field
- phase-0003 done (planner): first-match index for host-only runs; ≥10× target met
- phase-0004 done (planner): Cow host, borrowed defaults, per-minute clock memo; pre-resolved counters skipped with reason
- phase-0005 done (planner): with_counters_from, M-19/M-20, docs, final numbers; status in-review
- Gate D passed; status awaiting-acceptance
- user accepted 2026-11-28 and chose squash; all squash checks passed (clean tree, on the work branch, 12 Goal-ID commits, no merges, nothing on origin, origin/main at the baseline); squashed, merged to main, pushed; no tag
