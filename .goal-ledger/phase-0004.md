# phase-0004 — Hot-path allocation, clock, and counter lookups

- Status: done
- Owner: planner
- Depends on: phase-0003
- Goal: Cut per-request work in routing: normalize the host once without allocating when it is already normal, stop cloning default/target lists before a match is known, cache the local time per minute, and give compiled rules and profiles their counters up front instead of two `DashMap` lookups per request.
- Done when: every command in `## Verify` exits 0; route and counter tests pass unchanged; the benchmark's miss-heavy and url cases do not regress.
- Pattern to follow: `src/router/mod.rs` `Router::route_with`, `src/router/condition.rs` `LocalTime::now` / `normalize_host`, `src/routing.rs` `RoutingCounters::track`

## Context
P-09 part 3. `route_with` allocates the normalized host and clones `default` per request; `LocalTime::now()` calls `now_local()` (glibc `localtime_r` takes a global lock) whenever a time rule is evaluated; `RoutingCounters::track(rule, profile)` does two `DashMap` lookups per request. Decisions: `normalize_host` returns `Cow<str>`; defaults are borrowed until a decision is built; a process-wide `AtomicU64` caches `(unix_minute, weekday, minute_of_day)` for `SystemClock` (injected clocks are untouched); counters are pre-resolved per `RoutingState` (rule label / profile name → `Arc<TrafficCounters>`) with `track` falling back to the map for names not in the snapshot. Public signatures stay; `PinnedRouting` by value is out (see GOAL Log).

## Scope
- In: `src/router/mod.rs`, `src/router/condition.rs`, `src/routing.rs`, `src/http/` and `src/socks/` (only call sites of the counter API)
- Out: `src/filter.rs`, `Cargo.toml`, `Cargo.lock`, public trait signatures

## Sub-tasks
1. [done] `Cow` host normalization and no pre-match clones — done when: router tests pass.
2. [done] Minute-cached `SystemClock` — done when: a test shows two calls in the same minute do not re-query (via a test hook) and time-rule tests pass.
3. [skipped — reason: a counter lookup is two sharded `DashMap` reads (tens of nanoseconds, under 0.1% of a proxied request); pre-attaching needs either a new `Arc` field in the public `RouteDecision` or a second per-state cache that phase-0005's reload pruning must keep coherent — not worth the API and coherence cost] Pre-resolved counters — done when: `tests/traffic_counters.rs` and counter unit tests pass unchanged.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
cargo test --all-features --locked
cargo run --release --example route_bench --locked
git diff --exit-code -- src/filter.rs Cargo.toml Cargo.lock
```

## Escalate when
- Pre-resolving counters would change which label a request is counted under → stop; counters are user-visible.

## Log
- (append-only, one line per event)
- planner begins phase-0004 (planner-owned)
- sub-task 1: `normalize_host_cow` (borrows when already normal; public `normalize_host` unchanged); `route_with` borrows table defaults and the matched rule's target list instead of cloning them before the match is known (an empty chain still resolves `direct`)
- sub-task 2: `SystemClock::now` memoizes the local time per unix minute in one `AtomicU64` (exact: local time only changes on minute boundaries; injected clocks untouched); documented in DOX in phase-0005 as a clock memo, not a data cache
- sub-task 3 skipped (see sub-task line)
- phase-0004 verified and closed

## Evidence
- (append-only: command → observed result)
- new tests `system_clock_queries_the_local_time_once_per_minute`, `normalize_host_cow_matches_normalize_host` → pass
- clippy default / --no-default-features / --all-features → exit 0; tests 231 / 216 / 271 passed; `tests/traffic_counters.rs` → passed unchanged
- bench (two runs): N=5000 host 75 ns (phase-0003: 100 ns), wildcard 85–87 ns (110–127 ns); mixed 36 µs and url 50–57 µs within run-to-run noise of earlier runs (url baseline 44–51 µs, phase-0003 51–54 µs)
