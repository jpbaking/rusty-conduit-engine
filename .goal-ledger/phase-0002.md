# phase-0002 — Server TCP_NODELAY, HTTP/1.1 header-read timeout, and drain_timeout task abort

- Status: done
- Owner: executor
- Depends on: phase-0001
- Goal: Set `TCP_NODELAY` on accepted client sockets (`P-03`), add an HTTP/1.1 header-read timeout to defeat slowloris permit exhaustion (`B-06`), and abort remaining connection tasks when `drain_timeout` expires (`B-06`).
- Done when: every command in `## Verify` exits 0.
- Pattern to follow: `src/server.rs:595-626` (`accept_loop` socket options + task spawn) and `src/http/mod.rs:220-230` (`http1::Builder`)

## Context
Accepted client sockets currently leave Nagle's algorithm enabled, causing 40–200 ms stalls with Delayed-ACK on SOCKS5 handshakes and `CONNECT` responses.
Dedicated HTTP listeners have no header-read timeout, so silent TCP clients can hold `max_connections` permits indefinitely.
In `Server::run`, `tokio::time::timeout(drain_timeout, tracker.wait())` logs `"connection(s) aborted"` on timeout but does not actually abort the spawned tasks, leaving stuck tasks alive in long-lived embedders like `rusty-conduit-gui`.

## Scope
- In: `src/server.rs`, `src/http/mod.rs`
- Out: `src/http/mitm_tunnel.rs`, `src/net.rs`, `src/upstream/`, `src/router/`, `src/logging.rs`, `docs/`

## Sub-tasks
1. [done] In `src/server.rs` (`accept_loop`), call `let _ = stream.set_nodelay(true);` on every accepted client socket right before/alongside `ka.apply(&stream)`. Add a unit test in `src/server.rs` verifying `conn.stream.nodelay()` is `true` inside a handler — done when: `cargo test --locked nodelay` passes.
2. [done] In `src/http/mod.rs`, add `pub const DEFAULT_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);`, store `header_read_timeout: Option<Duration>` (default `Some(DEFAULT_HEADER_READ_TIMEOUT)`) on `HttpProxy`, and add `pub fn with_header_read_timeout(mut self, timeout: Option<Duration>) -> Self`. In `HttpProxy::serve`, configure `http1::Builder::new().timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(self.header_read_timeout)` (do not use `hyper_util` features not already enabled — verify `TokioTimer` compiles with existing `Cargo.toml`). Add a unit test proving a silent TCP client is closed after a short `header_read_timeout` (e.g., 100 ms) — done when: `cargo test --locked header_read_timeout` passes.
3. [done] In `src/server.rs`, track spawned connection tasks' `AbortHandle`s in an `Arc<DashMap<u64, tokio::task::AbortHandle>>` (with an `Arc<AtomicU64>` ID counter) alongside `TaskTracker`; each spawned task removes its own ID from the `DashMap` via a drop guard when it exits (whether normally or aborted). When `tokio::time::timeout(drain_timeout, tracker.wait())` times out in `Server::run`, abort every remaining handle in the `DashMap` (`for entry in abort_handles.iter() { entry.value().abort(); }`). Add a unit test in `src/server.rs` with a handler that awaits `std::future::pending::<()>()` while holding a drop-flag `Arc<AtomicBool>`, and assert that after `drain_timeout` expires `ShutdownReport { drained: false }` is returned AND the drop-flag becomes `true` — done when: `cargo test --locked drain_timeout_aborts` passes.

4. [done] Fix-up: run `cargo fmt` to repair the `cargo fmt --check` failure (import order in `src/server.rs` and a long line in the nodelay test recorder) — done when: `cargo fmt --check` exits 0.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
```

## Escalate when
- `hyper_util::rt::TokioTimer` is not enabled by the existing `Cargo.toml` features (do not edit `Cargo.toml` without planner approval) → stop, set Status: needs-human — reason: paste the compiler error.
- Any existing `src/server.rs` or `tests/` shutdown test regresses → stop, set Status: needs-human — reason: paste the failing test output.

## Log
- (append-only, one line per event)
- sub-task 1: `accept_loop` calls `stream.set_nodelay(true)` (debug-logs failure) before keepalive; Recorder records `conn.stream.nodelay()`; added `accepted_client_sockets_get_tcp_nodelay`
- sub-task 2: added `DEFAULT_HEADER_READ_TIMEOUT` (30s), `HttpProxy.header_read_timeout` + `with_header_read_timeout`; `serve` uses `http1::Builder::new().timer(TokioTimer::new()).header_read_timeout(..)`; `TokioTimer` compiled with the existing `hyper-util` `tokio` feature (Cargo.toml untouched); added `header_read_timeout_closes_silent_clients`
- sub-task 3: added `ConnectionTasks` (`Arc<AtomicU64>` ID + `Arc<DashMap<u64, AbortHandle>>`) and `AbortHandleGuard` drop guard; `Server::run` calls `abort_all()` when the drain times out; after inserting a handle, `accept_loop` removes it again if the task already finished (avoids a stale entry when the task ended before registration); added `drain_timeout_aborts_stuck_connection_tasks`
- Verify run 1: `cargo fmt --check` failed (formatting only); clippy x2 passed
- sub-task 4 (fix-up 1): ran `cargo fmt`; Verify run 2 passed in full

## Evidence
- (append-only: command → observed result)
- `cargo test --locked nodelay` → `server::tests::accepted_client_sockets_get_tcp_nodelay ... ok` (plus existing `direct_connects_with_nodelay_and_keepalive ... ok`); change at src/server.rs:636, test at src/server.rs:897
- `cargo test --locked header_read_timeout` → `http::tests::header_read_timeout_closes_silent_clients ... ok`, finished in 0.11s (100 ms timeout fired); src/http/mod.rs:123, :173, :238, test :585
- `cargo test --locked drain_timeout_aborts` → `server::tests::drain_timeout_aborts_stuck_connection_tasks ... ok` (asserts `ShutdownReport { drained: false }` and drop flag set); src/server.rs:72, :87, :577, :675, test :1152
- Verify run 1: `cargo fmt --check` → diff in src/server.rs (import order, line wrap); `cargo clippy --all-targets --locked -- -D warnings` → Finished, no warnings; `cargo clippy --all-targets --no-default-features --locked -- -D warnings` → Finished, no warnings
- Verify run 2 (after `cargo fmt`): `cargo fmt --check` → exit 0; both clippy runs → Finished, no warnings; `cargo test --locked` → 185+5+4+6+2+1+0 passed, 0 failed (203 total, was 200); `cargo test --no-default-features --locked` → 174+5+4+6+2+0+0 passed, 0 failed (191 total, was 188); existing shutdown tests `shutdown_drain_is_bounded` / `shutdown_drains_connections_that_honour_cancellation` still pass
