# phase-0002 — Indexed conditions and destination filter

- Status: done
- Owner: planner
- Depends on: phase-0001
- Goal: Make each compiled condition and the destination filter match in sub-linear time where the pattern allows it, with identical results.
- Done when: every command in `## Verify` exits 0, including equivalence tests that compare the new matchers with the old representation over pattern/host corpora.
- Pattern to follow: `src/router/condition.rs` `host_wildcard_regex` (the semantics every fast path must reproduce exactly) and `Condition::matches`

## Context
P-09 part 1. `Condition::Host` scans a `Vec`; `HostRegex` / `UrlRegex` run each `Regex` in turn; `HostWildcard` is one alternation regex; `BypassList` and `HostFilter` scan entries. Decisions: `Host` → `HashSet<Box<str>>`; `HostWildcard` → exact-name set + apex-and-subdomain suffix set (`*.d`) + subdomain-only suffix set (`**.d`) + a residual alternation regex for every other pattern (`?`, inner `*`, `|`, trailing `.*`), matched by walking the host's label suffixes; `HostRegex` / `UrlRegex` → `regex::RegexSet` (with the existing no-`routing` placeholder approach); `HostFilter` domain rules → exact and suffix sets with the same walk. Case and trailing-dot normalization stay where they are. No new dependencies.

## Scope
- In: `src/router/condition.rs`, `src/filter.rs`, `src/router/validate.rs` (only if compile paths need it), `src/router/mod.rs` (the `filter` field type and its construction only — added by the planner during the phase)
- Out: the rest of `src/router/mod.rs` (phase-0003), `src/routing.rs`, `Cargo.toml`, `Cargo.lock`

## Sub-tasks
1. [done] `Host` and `HostWildcard` fast paths with a residual regex — done when: an equivalence test over every wildcard form in the existing tests plus a generated corpus agrees with `host_wildcard_regex` for each host.
2. [done] `RegexSet` for `HostRegex` / `UrlRegex` (and the bypass wildcard list where applicable) — done when: existing condition tests pass unchanged and a set-vs-loop equivalence test passes.
3. [done] `HostFilter` exact / suffix sets — done when: existing filter tests pass and an equivalence test over generated rules agrees with the old scan.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
cargo run --release --example route_bench --locked
git diff --exit-code -- src/routing.rs Cargo.toml Cargo.lock
```

## Escalate when
- A pattern form cannot be classified with certainty → it goes to the residual regex (never guess).
- An existing test's expectation would have to change → stop; that is a behavior change needing a divergence decision.

## Log
- (append-only, one line per event)
- planner begins phase-0002 (planner-owned)
- constraint found before starting: consumers construct and pattern-match `HostFilter::{Allow, Block, BlockExcept}` with `Vec<FilterRule>`, so the public enum keeps its shape; the indexed filter is a private compiled form built where the router (and other hot callers) compile it

- design: `NameSet` (scanned when ≤ 8 names, hashed above, large variant boxed) for `Condition::Host` and the `host_wildcard` classes; `HostWildcardMatcher` keeps the full alternation regex as the reference (compiled first, so errors and the no-`routing` rejection are unchanged), classifies plain / `*.d` / `**.d` / `*` patterns into name sets matched by suffix, keeps every other pattern in a residual regex, and sends non-ASCII hosts to the full regex (Unicode case folding); `regex_list` combines `host_regex` / `url_regex` into one alternation when it builds, else keeps separate regexes; `CompiledFilter` indexes exact and `*.d` filter domains and shares `HostFilter::check_host`'s decision and message code (`check_host_with`), used by the router; the public `HostFilter` enum is unchanged
- scope amended: the router's `filter` field had to change type (`Option<HostFilter>` → `Option<CompiledFilter>`, 3 lines in src/router/mod.rs)
- bench note: hashing one name per rule made the 5000-rule host case slower (30.8 µs), hence `NameSet::Few`; a ~20% linear-scan cost remains in the host-only case (9.2 µs vs 7.8 µs, re-measured side by side) whose cause boxing and inlining did not change — phase-0003's index replaces that scan, so it is accepted here and re-checked at phase-0005
- `Condition::{Host, HostWildcard}` payload types changed (public enum, no consumer references its variants; only `RuleMatcher::compile` is used outside the engine) — divergence-ledger note at phase-0005
- follow-up (not in scope): `net.rs` direct-dial domain checks still scan the filter lists once per dial
- phase-0002 verified and closed

## Evidence
- (append-only: command → observed result)
- equivalence tests: `host_wildcard_fast_paths_agree_with_the_full_regex` (17 pattern forms incl. `?`, `|`-free inner `*`, `*.example.*`, non-ASCII and Kelvin-sign hosts, 2000 generated hosts, each pattern alone and all together), `combined_regex_lists_agree_with_separate_regexes`, `compiled_filter_agrees_with_check_host` (allow / block / block-except, 1500+ hosts incl. IPs and brackets) → all pass
- clippy default / --no-default-features / --all-features → exit 0; `cargo test --locked` → 228 passed; `--no-default-features` → 214 passed
- bench (release, side by side): baseline 5000 host 7.8 µs, wildcard 332–338 µs, mixed 54–55 µs, url 47–49 µs → after 9.1–9.6 µs, 36–39 µs (≈9×), 30–36 µs, 43–51 µs
