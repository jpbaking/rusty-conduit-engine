# phase-0007 — import: B-08 fixes, divergence ledger, and docs

- Status: done
- Owner: planner
- Depends on: phase-0006
- Goal: Fix the importer drift found in B-08 inside the engine copy, record every behavior change, and document the new features.
- Done when: every command in `## Verify` exits 0; tests cover each B-08 fix; `docs/DIVERGENCE-LEDGER.md` has entries for them; `DOX.md` Feature Map lists `import`, `test-support`, and `Serialize`.
- Pattern to follow: `src/import/airswitch.rs` `all_none!` / `pick!` macros and `merge()`; `src/import/mod.rs` `render_profile` / `rule_lines`

## Context
B-08 (`docs/OPTIMIZATION.md` row): the renderer omits `tls_insecure_skip_verify` (profiles) and `bypass_mitm` (rules); the `all_none!` / `pick!` macros omit `bypass_mitm`; `merge()` returns `None` whenever an outer and an inner nested `SwitchProfile` rule both set `host_wildcard`. Fix: exhaustive struct destructuring in the renderer and macros (so a new `RuleConfig` / `ProfileConfig` field fails to compile until handled), emit both fields, and conjoin overlapping condition fields where the rule language can express it (otherwise keep the current skip-with-warning). Each output change is a divergence from switch's former importer: record `M-17`+ with decision `fix`. Planner-owned: the merge semantics need judgment.

## Scope
- In: `src/import/mod.rs`, `src/import/airswitch.rs`, `docs/DIVERGENCE-LEDGER.md`, `DOX.md` (Feature Map and contract feature bullets), `docs/TODO.md` (remove done items only)
- Out: `Cargo.toml`, `Cargo.lock`, `src/routing.rs`, `src/router/`, the consumer repos

## Sub-tasks
1. [done] Renderer emits `tls_insecure_skip_verify = true` and `bypass_mitm = true` via exhaustive destructuring — done when: round-trip tests show both survive `convert` → `render_toml` → parse.
2. [done] `all_none!` / `pick!` cover `bypass_mitm` (exhaustively) — done when: a test rule with only `bypass_mitm` set is not treated as empty.
3. [done] `merge()` conjoins overlapping `host_wildcard` (and any other field it can) for nested SwitchProfile rules; unrepresentable combinations keep the warning — done when: a nested-profile fixture that used to drop the rule now imports it, and a truly unrepresentable one still warns.
4. [done] Divergence entries (`M-17`+, `fix`) and DOX Feature Map / feature bullets for `import`, `test-support`, and routing `Serialize` — done when: `grep -c '^| M-17' docs/DIVERGENCE-LEDGER.md` prints `1` and `grep -c 'import' DOX.md` grows.

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
git diff --exit-code -- Cargo.toml Cargo.lock src/routing.rs src/router
```

## Escalate when
- A B-08 fix would change output for `.bak` files that import cleanly today (not just the buggy cases) → ask the user before keeping it.

## Log
- (append-only, one line per event)
- planner begins phase-0007 (planner-owned)
- finding: the `all_none!` / `pick!` macros named in B-08 no longer exist; the nested-rule AND is `merge()` with its `take!` macro. AirSwitch backups never produce `bypass_mitm` or `tls_insecure_skip_verify` (nor `password_env` / `password_file`), so the renderer gaps are latent; the fix is exhaustive destructuring so a new field cannot be dropped silently, plus the merge overlap

- sub-task 1: `render_profile` and `rule_lines` destructure `ProfileConfig` / `RuleConfig` exhaustively and now emit `password_env`, `password_file`, `tls_insecure_skip_verify`, `bypass_mitm`
- sub-task 2: `merge()` destructures `RuleConfig` exhaustively (replacing the `take!` macro); `bypass_mitm` is ORed
- sub-task 3: `and_field` merges equal values on any matcher field; `and_host_wildcard` + `wildcard_covers` keep the narrower side when every pattern is covered (equal, `*`, `*.d` / `**.d` over plain names; `?`, `|`, inner `*` never covered); unrelated patterns keep the existing "cannot be ANDed" skip
- escalation check: no output change for .bak files that imported cleanly before (only previously skipped nested rules change; the renderer's new fields are never set by AirSwitch; `rendering_is_deterministic` and every copied test still pass)
- sub-task 4: M-17 (importer B-08, fix) and M-18 (shared file sections: Debug redaction, cert_cache_max accepted by all CLIs, keep); DOX Feature Map gains test-support and import entries and the routing line mentions Serialize; docs/TODO.md had nothing completed by this goal
- phase-0007 verified and closed

## Evidence
- (append-only: command → observed result)
- `cargo test --features import --locked import::` → 22 passed (new: `nested_wildcards_inside_the_outer_pattern_now_import`, `wildcard_coverage_is_conservative`, `merge_keeps_equal_values_and_bypass_mitm`, `renderer_writes_every_profile_and_rule_field`; existing `merge_rejects_same_field` still passes)
- fmt clean; clippy default / --no-default-features / --all-features exit 0; tests 225 / 213 / 265 passed; PURE_RUST_CRYPTO_OK; `git diff --exit-code -- Cargo.toml Cargo.lock src/routing.rs src/router` → exit 0
- `grep -c '^| M-17' docs/DIVERGENCE-LEDGER.md` → 1; `grep -c 'import' DOX.md` → 4 (was 2)
