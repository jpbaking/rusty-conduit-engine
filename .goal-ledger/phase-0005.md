# phase-0005 — Passive upstream failover cooldown and single lazy-timeout dial wrapper

- Status: done
- Owner: executor
- Depends on: phase-0004
- Goal: Add bounded passive failure cooldown (default 3 s, `Duration::ZERO` disables) to `ProfileRegistry` / `FailoverDialer` for multi-target chains (`P-07`), and make `with_timeout` use a lazy error-context closure so successful dials allocate zero error strings (`P-07`).
- Done when: every command in `## Verify` exits 0.
- Pattern to follow: `src/upstream/mod.rs:294-308` (`with_timeout`) and `src/upstream/mod.rs:464-650` (`ProfileRegistry`, `FailoverDialer`)

## Context
When a primary target in a multi-target failover chain (`["vpn", "ssh", "direct"]`) is dead, every incoming connection stalls for the full `connect_timeout` (10 s) before failing over.
Adopted design (from `docs/OPTIMIZATION.md`): `ProfileRegistry` tracks lightweight per-profile atomic failure timestamps (`AtomicU64` millis since registry creation, `0` = healthy) with a configurable `failover_cooldown` defaulting to `3s` (`Duration::ZERO` disables).
Rules:
- Single-target chains (`self.entries.len() <= 1`) NEVER skip.
- Only dial/timeout failures trigger cooldown (never `RouteBlocked` / `AccessDenied`).
- During cooldown, subsequent connections on a multi-target chain skip straight to the next target; if ALL targets in a multi-target chain are currently in cooldown, fall back to trying them in order rather than failing without dialing.
- Once cooldown elapses, atomically claim the probe slot (updating the timestamp to `now` via `compare_exchange` before dialing) so only one concurrent connection probes the primary while others keep failing over; on success, clear the timestamp back to `0`.
- Keep `DialerOptions` backwards-compatible with existing struct literals (`failover_cooldown` via builder method or `ProfileRegistry::with_failover_cooldown`). No sockets or DNS entries are cached.

## Scope
- In: `src/upstream/mod.rs`
- Out: `src/server.rs`, `src/http/`, `src/socks/`, `src/net.rs`, `src/router/`, `src/logging.rs`, `docs/`

## Sub-tasks
1. [done] In `src/upstream/mod.rs`, change `with_timeout` to accept `what: impl FnOnce() -> String` (lazy closure evaluated only on timeout, zero allocation on `Ok`) and update all internal callers (`src/upstream/mod.rs` and any `src/upstream/*.rs` callers — check with `grep -rn 'with_timeout' src/upstream/`). Also avoid computing `target = host_port(host, port)` before it is needed or keep it lazy on error/debug — done when: `cargo test --locked upstream` passes.
2. [done] In `src/upstream/mod.rs`, add `pub const DEFAULT_FAILOVER_COOLDOWN: Duration = Duration::from_secs(3);`, a small `CooldownState` (`epoch: Instant`, per-profile `Arc<AtomicU64>`), `ProfileRegistry::with_failover_cooldown(mut self, cooldown: Duration) -> Self`, and `ProfileRegistry::failover_cooldown(&self) -> Duration` (defaulting to `DEFAULT_FAILOVER_COOLDOWN` in `ProfileRegistry::from_profiles` and `Default`). Wire `FailoverDialer` created by `ProfileRegistry::chain` / `chain_for` to consult and update the shared per-profile cooldown atomics according to the rules in `## Context` (skip in cooldown only when `entries.len() > 1` and at least one other entry is not in cooldown; single-target never skips; `RouteBlocked` / `AccessDenied` never mark cooldown; CAS-claim probe after cooldown elapses; clear to `0` on success; log skipped target at `DEBUG` if logger is set) — done when: `cargo test --locked failover` passes.
3. [done] Add unit tests in `src/upstream/mod.rs` covering: (a) multi-target chain skips a dead primary during cooldown without invoking its `connect`; (b) after cooldown elapses, the primary is probed again and recovers on success; (c) single-target chain never skips even after failure; (d) `with_failover_cooldown(Duration::ZERO)` disables cooldown; (e) if every target in a multi-target chain is in cooldown, targets are still dialed rather than returning empty failure — done when: `cargo test --locked cooldown` passes.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
```

## Escalate when
- Any existing `src/upstream/mod.rs` failover test expects a dead primary in a multi-target chain to be dialed twice back-to-back within 3 s → update that test to use `with_failover_cooldown(Duration::ZERO)` ONLY if the test is specifically testing repeated failure without cooldown; if the test is testing something else, stop, set Status: needs-human — reason: name the test and explain.
- Any `src/upstream/*.rs` file outside `src/upstream/mod.rs` calls `with_timeout` and needs more than a trivial `|| format!(...)` closure update → stop, set Status: needs-human — reason: list the call sites.

## Log
- (append-only, one line per event)
- sub-task 1: `with_timeout` now takes `what: impl FnOnce() -> String` (evaluated only on timeout); callers in mod.rs, http.rs (2), socks4a.rs, socks5.rs switched to `|| format!(...)` closures (trivial); `FailoverDialer::dial` no longer precomputes `host_port` (computed only in timeout/debug/error paths)
- sub-task 2: added `DEFAULT_FAILOVER_COOLDOWN` (3 s), private `CooldownState` (epoch Instant + cooldown), per-profile `Arc<AtomicU64>` marks in `ProfileRegistry` (`insert` creates them), `with_failover_cooldown` / `failover_cooldown`, manual `Default`; `chain`/`chain_for` hand marks to `FailoverDialer` unless cooldown is zero; `dial` skips cooling entries (multi-target only, only if another entry is not cooling), CAS-claims the probe after expiry, marks on dial/timeout failure (not RouteBlocked, not AccessDenied), clears on success, DEBUG-logs skips; skipped entries are retried last (in order) only after every live entry failed
- sub-task 3: added `CountingDialer` test helper and 6 `failover_cooldown_*` tests (default 3 s, skip dead primary, probe+recover after expiry, single-target never skips, ZERO disables, all-cooling still dials)
- Verify: all five commands passed

## Evidence
- (append-only: command → observed result)
- `cargo test --locked upstream` → test result: ok. 29 passed; 0 failed (lib), all other targets ok
- `cargo test --locked failover` → ok. 1 passed (http::mitm_tunnel::tests::chain_key_identifies_failover_chains) before tests added; after sub-task 3 the 6 `failover_cooldown_*` tests also match
- `cargo test --locked cooldown` → ok. 6 passed; 0 failed
- `cargo fmt --check` → exit 0
- `cargo clippy --all-targets --locked -- -D warnings` → Finished, no warnings
- `cargo clippy --all-targets --no-default-features --locked -- -D warnings` → Finished, no warnings
- `cargo test --locked` → lib 202 passed; integration 5+4+6+2+1 passed; 0 failed
- `cargo test --no-default-features --locked` → lib 191 passed; integration 5+4+6+2 passed; 0 failed
