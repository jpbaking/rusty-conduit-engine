# phase-0003 — First-match index across host-only rule runs

- Status: done
- Owner: planner
- Depends on: phase-0002
- Goal: Route large imported configs without testing every rule: index consecutive rules whose only conditions are `host` / `host_wildcard`, and find the first matching rule through that index while keeping first-match order.
- Done when: every command in `## Verify` exits 0; a randomized equivalence test (deterministic seed, thousands of rules and requests, mixed rule kinds including disabled, `match_all`, and non-host rules between host-only runs) shows the indexed `first_match` equals a linear scan; the 5000-rule host and wildcard benchmark cases are at least 10× faster than baseline.
- Pattern to follow: `src/router/mod.rs` `Table::compile` / `Table::first_match`

## Context
P-09 part 2 ("jump index"). A table is split into segments: maximal runs of host-only, enabled, non-`match_all` rules become an index segment (exact host → lowest rule index; suffix → lowest rule index; residual patterns → (rule index, matcher) list); every other rule is its own linear segment. `first_match` walks segments in order; inside an index segment the answer is the minimum of the exact lookup, the suffix walk, and the first matching residual rule below that minimum. The old linear `first_match` stays as `first_match_linear` for tests. Rule indices, labels, and `bypass_mitm` reporting must not change.

## Scope
- In: `src/router/mod.rs`, `src/router/condition.rs` (helpers exposed for the index)
- Out: `src/routing.rs`, `src/filter.rs`, `Cargo.toml`, `Cargo.lock`

## Sub-tasks
1. [done] Segment builder and indexed `first_match` — done when: all existing router tests pass unchanged.
2. [done] Randomized equivalence test against `first_match_linear` — done when: it passes for 3 seeds.
3. [done] Benchmark — done when: Evidence shows host / wildcard 5000-rule cases ≥ 10× faster than phase-0001's baseline.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
cargo run --release --example route_bench --locked
git diff --exit-code -- src/routing.rs src/filter.rs Cargo.toml Cargo.lock
```

## Escalate when
- The 10× target is not reached after the index is in place → record the numbers and ask the user whether to accept them.

## Log
- (append-only, one line per event)
- planner begins phase-0003 (planner-owned)
- design: `RuleMatcher::host_only` classifies rules (disabled → never matches, single `host` / `host_wildcard` condition → keys, anything else incl. `match_all` → evaluated alone); `segments()` groups maximal runs of host-only rules (≥ 8, `MIN_INDEXED_RUN`) into `HostRun` maps (exact, `*.d`, `**.d` → lowest rule index, `*` → lowest index, residual-pattern rules checked only below the best index); non-ASCII (or not-lowercase) hosts scan the run, preserving the matcher's Unicode semantics; `first_match_linear` kept as the test reference
- phase-0003 verified and closed

## Evidence
- (append-only: command → observed result)
- `cargo test --locked --lib indexed_first_match` → ok (seeds 1, 2, 3; 1200 generated rules per table mixing exact, `*.d`, `**.d`, residual `dev-*` / `h?st`, `*`, disabled, `match_all`, port-qualified, `host_regex`, multi-host; 3006 hosts × 2 ports incl. a Kelvin-sign host and the empty host)
- clippy default / --no-default-features / --all-features → exit 0; tests: default 229, --no-default-features 214, --all-features 269 passed; `git diff --exit-code -- src/routing.rs src/filter.rs Cargo.toml Cargo.lock` → exit 0
- bench (release, two runs): N=5000 host 100–102 ns (baseline 7779 → ≈77×), wildcard 110–127 ns (baseline 337235 → ≈2600×); N=1000 host 106–110 ns, wildcard 108–113 ns; mixed and url unchanged (mixed alternates kinds every rule so never forms a run; url rules are not host-only) — the ≥10× target is met
