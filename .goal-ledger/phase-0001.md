# phase-0001 — Routing benchmark example and baseline

- Status: done
- Owner: executor
- Depends on: none
- Goal: Add a dependency-free benchmark example that times `Router::route` over generated rule sets, and record the baseline numbers before any optimization.
- Done when: `cargo run --release --example route_bench` prints the table described below and the numbers are recorded in this phase's `## Evidence`.
- Pattern to follow: `src/router/mod.rs` tests `router(text)` helper (around line 582) for the idea only — it calls the crate-private `routing::test_support::input`, which an example cannot use; build `RoutingInput { profiles, routing, ..RoutingInput::default() }` yourself from a parsed TOML struct

## Context
P-09 claims large speedups for imported configs with thousands of rules. Every later phase is judged against these numbers, so they must come from the unmodified router. No new dependencies: use `std::time::Instant`, a hand-written deterministic generator (a 64-bit LCG: `state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)`), and build the config as a TOML string parsed with the existing dev-dependency `toml` into `RoutingConfig` / profiles, then `Router::from_input`. Examples may use dev-dependencies.

## Scope
- In: `examples/route_bench.rs` (new)
- Out: everything under `src/`, `Cargo.toml`, `Cargo.lock`, `tests/`

## Sub-tasks
1. [done] Create `examples/route_bench.rs`. It declares one profile `p` (`type = "http"`, `host = "127.0.0.1"`, `port = 3128`) and, for each N in `[100, 1000, 5000]`, builds four configs of N `[[routing.rules]]` (all `target = "p"`, `[routing].default = "direct"`): **host** — rule i has `host = "h{i}.example"`; **wildcard** — rule i has `host_wildcard = "*.w{i}.example"`; **mixed** — rule i cycles through `host = "h{i}.example"`, `host_wildcard = "*.w{i}.example"`, `host_regex = "^r{i}\\."`, `cidr = "10.{i/256 % 256}.{i % 256}.0/24"`; **url** — rule i has `url_wildcard = "*://u{i}.example/*"`. Requests: 10 000 per config from the LCG (seed 42): 50% hit a random rule's host form (`h{k}.example`, `x.w{k}.example`, `r{k}.example`, `10.a.b.1`, or url `http://u{k}.example/path` with host `u{k}.example`), 50% miss (`miss{k}.other`) — done when: `cargo build --release --example route_bench --locked` succeeds.
2. [done] For each (N, kind) run every request once to warm up, then time a second pass and print one line per case: `N=<n> kind=<kind> ns/route=<mean>` (mean over the 10 000 timed routes, integer nanoseconds); also print `rules compiled in <ms> ms` per config. Use `RouteRequest::new(Inbound::Http, host, 80)` and set `url` for the url kind (`RouteRequest` has a public `url` field; set `scheme` to `http`) — done when: `cargo run --release --example route_bench --locked` prints 12 `ns/route` lines.
3. [done] Run it twice and record both outputs in `## Evidence` (the planner compares later phases against the slower of the two runs per line) — done when: both outputs are in Evidence.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo run --release --example route_bench --locked
git diff --exit-code -- src Cargo.toml Cargo.lock tests
```

## Escalate when
- A generated config fails to compile in `Router::from_input` → stop, set Status: needs-human — reason: paste the error and the first offending rule.
- `RouteRequest` lacks a field this phase relies on → stop, set Status: needs-human — reason: name it (do not change `src/`).

## Log
- (append-only, one line per event)
- sub-task 1: created `examples/route_bench.rs` with TOML config generator, deterministic LCG (seed 42) request generator, warm-up + timed pass, and `N=<n> kind=<kind> ns/route=<mean>` output
- sub-task 2: ran `cargo run --release --example route_bench --locked`; printed 12 `ns/route` lines with compile times
- sub-task 3: ran second pass of `route_bench` and recorded both baseline runs in Evidence; ran all `## Verify` commands and all passed

- planner accepted phase-0001: example reviewed (LCG requests, warm-up + timed pass, black_box), Verify output in Evidence; baseline per line = slower run (5000 host 7779 ns, 5000 wildcard 337235 ns)

## Evidence
- (append-only: command → observed result)
- `cargo build --release --example route_bench --locked` → exit 0 (`Finished release profile [optimized] target(s) in 16.43s`)
- `cargo run --release --example route_bench --locked` (run 1) → exit 0, 12 lines:
  `N=100 kind=host ns/route=528 (rules compiled in 0 ms)`
  `N=100 kind=wildcard ns/route=1372 (rules compiled in 4 ms)`
  `N=100 kind=mixed ns/route=606 (rules compiled in 1 ms)`
  `N=100 kind=url ns/route=744 (rules compiled in 0 ms)`
  `N=1000 kind=host ns/route=1794 (rules compiled in 0 ms)`
  `N=1000 kind=wildcard ns/route=19018 (rules compiled in 32 ms)`
  `N=1000 kind=mixed ns/route=7862 (rules compiled in 9 ms)`
  `N=1000 kind=url ns/route=8238 (rules compiled in 3 ms)`
  `N=5000 kind=host ns/route=7779 (rules compiled in 1 ms)`
  `N=5000 kind=wildcard ns/route=337235 (rules compiled in 156 ms)`
  `N=5000 kind=mixed ns/route=57492 (rules compiled in 45 ms)`
  `N=5000 kind=url ns/route=51179 (rules compiled in 15 ms)`
- `cargo run --release --example route_bench --locked` (run 2) → exit 0, 12 lines:
  `N=100 kind=host ns/route=295 (rules compiled in 0 ms)`
  `N=100 kind=wildcard ns/route=1340 (rules compiled in 4 ms)`
  `N=100 kind=mixed ns/route=554 (rules compiled in 0 ms)`
  `N=100 kind=url ns/route=670 (rules compiled in 0 ms)`
  `N=1000 kind=host ns/route=1507 (rules compiled in 0 ms)`
  `N=1000 kind=wildcard ns/route=18628 (rules compiled in 30 ms)`
  `N=1000 kind=mixed ns/route=7667 (rules compiled in 8 ms)`
  `N=1000 kind=url ns/route=7833 (rules compiled in 2 ms)`
  `N=5000 kind=host ns/route=7436 (rules compiled in 0 ms)`
  `N=5000 kind=wildcard ns/route=177968 (rules compiled in 150 ms)`
  `N=5000 kind=mixed ns/route=51505 (rules compiled in 43 ms)`
  `N=5000 kind=url ns/route=43971 (rules compiled in 14 ms)`
- `cargo fmt --check` → exit 0
- `cargo clippy --all-targets --all-features --locked -- -D warnings` → exit 0
- `cargo clippy --all-targets --no-default-features --locked -- -D warnings` → exit 0
- `git diff --exit-code -- src Cargo.toml Cargo.lock tests` → exit 0
