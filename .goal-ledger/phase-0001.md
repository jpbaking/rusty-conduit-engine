# phase-0001 — Bind listeners with a 4096 backlog

- Status: done
- Owner: executor
- Depends on: none
- Goal: Make `Server::bind` create every listener with a requested backlog of 4096, keeping all other socket behaviour the same as `TcpListener::bind`.
- Done when: the full verification in `## Verify` passes and the three new tests exist in `src/server.rs`.
- Pattern to follow: `src/server.rs:771-790` (`bind_conflict_reports_role_and_addr`, for test shape) and `src/net.rs:419` (`let Ok(listener) = TcpListener::bind("[::1]:0").await else { return; }`, for skipping when IPv6 is unavailable)

## Context
`Server::bind` (`src/server.rs:344-375`) calls `tokio::net::TcpListener::bind`, which goes through mio and always asks the kernel for a backlog of 128. The kernel uses the smaller of the request and its own cap (`net.core.somaxconn`, 4096 by default on Linux ≥ 5.4; `kern.ipc.somaxconn`, 128 on macOS), so today every platform gets 128. Requesting 4096 lets the kernel cap decide. mio also sets `SO_REUSEADDR` on non-Windows platforms before binding; the new helper must do the same so restarts with sockets in `TIME_WAIT` keep working. The planner prototyped exactly this helper against tokio 1.54.1: it binds v4 and v6, keeps `SO_REUSEADDR`, and still fails with `AddrInUse` on a taken port.
No public API changes except the new public constant. Do not make the backlog configurable.
Add this code to `src/server.rs`, next to the other constants near the top of the file and above `impl Server`:
```rust
/// Listen backlog requested for every listener. The kernel caps it at its own limit
/// (`net.core.somaxconn` on Linux, `kern.ipc.somaxconn` on macOS); `TcpListener::bind`
/// would request only 128.
pub const LISTEN_BACKLOG: u32 = 4096;

/// Bind `addr` like `TcpListener::bind` (with `SO_REUSEADDR` on non-Windows platforms),
/// but request a listen backlog of [`LISTEN_BACKLOG`].
fn bind_listener(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    #[cfg(not(windows))]
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(LISTEN_BACKLOG)
}
```

## Scope
- In: `src/server.rs`
- Out: `src/lib.rs`, `src/mux.rs`, `src/net.rs`, `Cargo.toml`, `Cargo.lock`, `tests/`, `docs/`, `DOX.md`, every other file

## Sub-tasks
1. [done] Add `TcpSocket` to the `use tokio::net::{...}` line (`src/server.rs:28`), add the constant and helper from `## Context`, and in `Server::bind` replace `TcpListener::bind(addr).await.map_err(bind_err)?` (`src/server.rs:363`) with `bind_listener(addr).map_err(bind_err)?` — done when: `cargo build --locked` succeeds and `grep -n 'TcpListener::bind(addr)' src/server.rs` prints nothing.
2. [done] In the `mod tests` of `src/server.rs`, add `listen_backlog_is_4096` (asserts `LISTEN_BACKLOG == 4096`) and a `#[cfg(unix)]` async test `listeners_keep_reuse_address` that binds `Server::bind(ListenPlan::Separate { http: Some(loopback(0)), socks: None }, ...)` and asserts `socket2::SockRef::from(&server.listeners[0].2).reuse_address().unwrap()` is `true` — done when: `cargo test --locked --lib server::tests::listen` and `cargo test --locked --lib server::tests::listeners_keep` each run and pass 1 test.
3. [done] Add a `#[cfg(target_os = "linux")]` async test `backlog_queues_more_than_128_connections`: read `/proc/sys/net/core/somaxconn` (return early if unreadable or below 300); bind a `Server` on `loopback(0)` but do not call `run`, so nothing accepts; connect 300 `TcpStream`s to its HTTP address one after another, each wrapped in `tokio::time::timeout(Duration::from_millis(200), ...)`, keeping the streams alive in a `Vec`; assert that more than 128 connected — done when: `cargo test --locked --lib server::tests::backlog` passes (on macOS the test is compiled out; record that in `## Evidence`).
4. [done] Add `#[tokio::test] async fn binds_ipv6_loopback()`: skip with the `src/net.rs:419` pattern if `[::1]:0` cannot be bound by `TcpListener`, otherwise `Server::bind` a `ListenPlan::Separate` with `http: Some("[::1]:0".parse().unwrap())` and assert `local_http_addr()` is an IPv6 address — done when: `cargo test --locked --lib server::tests::binds_ipv6` passes.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
python3 scripts/check-pure-rust-crypto.py
! grep -n 'TcpListener::bind(addr)' src/server.rs
```

## Escalate when
- `bind_conflict_reports_role_and_addr` or any existing test fails after the switch to `bind_listener` → stop, set Status: needs-human — reason: paste the failing test and error.
- `socket2::SockRef` is not usable from the unit tests (for example the dev-dependency features differ from what `Cargo.toml` line 61 shows) → stop, set Status: needs-human — reason: paste the compiler error.
- Any other file needs to change to make the build or tests pass → stop, set Status: needs-human — reason: name the file and why.

## Log
- (append-only, one line per event)
- sub-task 1: imported `TcpSocket`, added `LISTEN_BACKLOG` and `bind_listener`, and updated `Server::bind` in `src/server.rs`
- sub-task 2: added `listen_backlog_is_4096` and `listeners_keep_reuse_address` tests in `src/server.rs`
- sub-task 3: added Linux-only `backlog_queues_more_than_128_connections` test in `src/server.rs`
- sub-task 4: added `binds_ipv6_loopback` test in `src/server.rs`
- verify: ran all `## Verify` commands; all passed

## Evidence
- (append-only: command → observed result)
- `cargo build --locked && ! grep -n 'TcpListener::bind(addr)' src/server.rs` → exit 0 (`Finished dev profile`, no `TcpListener::bind(addr)` matches)
- `cargo test --locked --lib server::tests::listen && cargo test --locked --lib server::tests::listeners_keep` → exit 0 (2 passed: `listen_backlog_is_4096`, `listeners_keep_reuse_address`; then 1 passed: `listeners_keep_reuse_address`)
- `cargo test --locked --lib server::tests::backlog` → exit 0 (0 passed on macOS; `#[cfg(target_os = "linux")]` test compiled out as expected)
- `cargo test --locked --lib server::tests::binds_ipv6` → exit 0 (1 passed: `binds_ipv6_loopback`)
- `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo clippy --all-targets --no-default-features --locked -- -D warnings && cargo test --locked && cargo test --no-default-features --locked && python3 scripts/check-pure-rust-crypto.py && ! grep -n 'TcpListener::bind(addr)' src/server.rs` → exit 0 (fmt ok, clippy default & no-default-features ok, tests default: 164 unit + 4 integration passed, no-default-features: 153 unit + 3 integration passed, `PURE_RUST_CRYPTO_OK`, no `TcpListener::bind(addr)` matches)
- planner fix-up after review: the backlog test now asserts all 300 connections queued (`> 128` also passed with the old backlog, because Linux queues backlog + 1)
