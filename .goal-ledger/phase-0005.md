# phase-0005 — Reload-safe counters, divergence entries, docs, and after-numbers

- Status: done
- Owner: planner
- Depends on: phase-0004
- Goal: Give embedders a reload path that keeps counters only for profiles and rules that still exist and resets an unnamed rule's counter when the rule at that position changed (B-07 engine support), record every behavior change, document the index, and publish before/after benchmark numbers.
- Done when: every command in `## Verify` exits 0; reload counter tests cover pruning and the unnamed-rule reset; divergence entries exist for every behavior change of this goal; `DOX.md` and `src/router/DOX.md` describe the index.
- Pattern to follow: `src/routing.rs` `RoutingState::with_counters`; `../rusty-conduit-switch/src/reload.rs:256-270` (how switch carries counters across a reload today; read-only)

## Context
B-07: switch passes the previous `RoutingCounters` to the new state on every reload, so counters of removed profiles and rules live forever, and an unnamed rule keyed `[routing] rule #N` inherits another rule's totals when rules are inserted or reordered. Decision: add `RoutingState::with_counters_from(previous: &RoutingState)`, which shares the previous counters, drops entries whose profile or rule label is absent from the new router (keeping `direct`, `block`, and the default label), and resets an unnamed rule's counter when its compiled rule's fingerprint (a hash of its `RuleConfig`, now `Serialize`) differs from the previous router's rule with that label. `with_counters` keeps its current meaning. Switch adopts the new call in Phase 3b.

## Scope
- In: `src/routing.rs`, `src/router/mod.rs` (fingerprint on compiled rules), `docs/DIVERGENCE-LEDGER.md`, `DOX.md`, `src/router/DOX.md`, `docs/TODO.md`
- Out: `Cargo.toml`, `Cargo.lock`, the consumer repos

## Sub-tasks
1. [done] `with_counters_from` with pruning and the unnamed-rule reset — done when: unit tests cover removed profile, removed rule, unchanged unnamed rule (kept), and changed unnamed rule (reset).
2. [done] Divergence entries for phases 0002–0005 (expected: none for matching, which must be identical; one for the new reload API; any found difference) — done when: entries exist and name their tests.
3. [done] Docs and final benchmark table (baseline vs after, both runs) in Evidence — done when: the DOX files mention the index and `with_counters_from`.

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
cargo run --release --example route_bench --locked
git diff --exit-code -- Cargo.toml Cargo.lock
```

## Escalate when
- The fingerprint would reset counters on reloads where nothing changed → stop and fix before continuing (counters are user-visible).

## Log
- (append-only, one line per event)
- planner begins phase-0005 (planner-owned)
- sub-task 1: `CompiledRule` gains `named` and a `fingerprint` (DefaultHasher over the rule's serde_json form; fixed SipHash keys, stable within a process); `Router::rule_labels`; `RoutingCounters::retain`; `RoutingState::with_counters_from(&previous)` keeps named rules, the default label, unchanged unnamed rules, and profiles the new router or registry knows
- sub-task 2: M-19 (P-09 performance, keep; notes the `Condition` payload change) and M-20 (new reload API, option)
- sub-task 3: DOX (contract note on the clock memo, Feature Map for the index, `CompiledFilter`, `with_counters_from`, the benchmark), src/router/DOX.md (index contract and Feature Map), docs/TODO.md (net.rs filter scan, PinnedRouting, URL/mixed rules still linear)
- found while benchmarking: one segment per non-indexed rule cost ≈10% on URL-only tables; consecutive non-indexed rules now form one linear range (URL back at baseline, mixed 30 µs vs 51–57 µs baseline)
- phase-0005 verified and closed

## Evidence
- (append-only: command → observed result)
- `cargo test --locked --lib with_counters_from` → ok (unchanged reload keeps all; removed profile `b` and moved unnamed rules #2/#3 reset; named rule, default label, built-in `direct` kept)
- Full verification → exit 0 (fmt; clippy default / no-default / all-features; 721 tests across the three test runs; PURE_RUST_CRYPTO_OK); `git diff --exit-code -- Cargo.toml Cargo.lock` → exit 0
- final benchmark (release, two runs) vs baseline (slower phase-0001 run): N=5000 host 73–76 ns vs 7779 ns (≈100×); wildcard 86 ns vs 337235 ns (≈3900×); mixed 30.2–30.4 µs vs 57.5 µs; url 44.0–45.0 µs vs 51.2 µs; N=1000 host/wildcard ≈80 ns vs 1.8 µs / 19.0 µs
