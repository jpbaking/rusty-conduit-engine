# phase-0003 — Happy Eyeballs dialing, 32 KiB relay, vectored writes, and PrefixedStream cursor

- Status: done
- Owner: executor
- Depends on: phase-0002
- Goal: Add RFC 8305 Happy Eyeballs dual-stack dialing (`P-06`), raise `relay_bidirectional` buffers to 32 KiB (`P-10`), forward `poll_write_vectored`/`is_write_vectored` across stream wrappers (`P-10`), and switch `PrefixedStream::poll_read` to a cursor without `initialize_unfilled()` or `Vec::drain` (`P-10`).
- Done when: every command in `## Verify` exits 0.
- Pattern to follow: `src/net.rs:128-340` (`connect_upstream_filtered`, `relay_bidirectional`, `PrefixedStream`)

## Context
`connect_upstream_filtered` currently tries resolved addresses sequentially, so a blackholed IPv6 route consumes the entire 10 s connect timeout before IPv4 is ever tried. Happy Eyeballs (RFC 8305) interleaves IPv6 and IPv4 candidates and staggers subsequent attempts by 250 ms (or immediately on connect error), returning the first connected socket while dropping remaining attempts.
`relay_bidirectional` uses Tokio's 8 KiB default buffer; raising it to 32 KiB halves syscall overhead on bulk transfers. `PrefixedStream`, `UpstreamStream`, and `CountingStream` do not forward `poll_write_vectored`/`is_write_vectored`, and `PrefixedStream::poll_read` zeroes the full `ReadBuf` with `initialize_unfilled()` and shifts bytes via `Vec::drain(..n)`.
Never cache DNS lookups (fixed invariant: every dial resolves afresh).

## Scope
- In: `src/net.rs`, `src/upstream/mod.rs` (`UpstreamStream` `AsyncWrite` impl only), `src/http/tunnel.rs` (`CountingStream` `AsyncWrite` impl only)
- Out: `src/server.rs`, `src/router/`, `src/mitm/`, `src/logging.rs`, `docs/`

## Sub-tasks
1. [done] In `src/net.rs`, add `const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(250);`, a helper `fn interleave_addrs(addrs: Vec<SocketAddr>) -> Vec<SocketAddr>` that alternates IPv6 and IPv4 addresses (preserving relative order within each family, starting with the first address's family), and an async helper `connect_happy_eyeballs(candidates: Vec<SocketAddr>, deadline: Instant, host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream>` that starts the first connect immediately and staggers subsequent connects by `HAPPY_EYEBALLS_DELAY` (advancing immediately if the active attempt errors), returning the first `Ok(TcpStream)` and dropping/cancelling remaining futures. Use it in `connect_upstream_filtered` for both `filter = Some(...)` (multi-candidate) and `filter = None` (when `host` is not an IP literal, resolve via `lookup_host` within `deadline` and call `connect_happy_eyeballs`; for IP literals keep single `TcpStream::connect`). Add unit tests for `interleave_addrs` and a loopback Happy Eyeballs test (unreachable first port + listening second port succeeds quickly) — done when: `cargo test --locked happy_eyeballs` and `cargo test --locked interleave_addrs` pass.
2. [done] In `src/net.rs`, update `relay_bidirectional` to call `tokio::io::copy_bidirectional_with_sizes(client, upstream, 32 * 1024, 32 * 1024).await` — done when: `grep -c 'copy_bidirectional_with_sizes' src/net.rs` prints 1.
3. [done] In `src/net.rs`, update `PrefixedStream<S>` to track `pos: usize` alongside `buffer: Vec<u8>`: in `poll_read`, copy `min(remaining, buf.remaining())` bytes via `buf.put_slice(&this.buffer[this.pos..this.pos + n])`, advance `this.pos += n`, and clear `buffer` + reset `pos = 0` once drained; update `buffered()` to return `self.buffer.len() - self.pos` and `into_inner()` to return `(self.inner, self.buffer.split_off(self.pos))` (or `Vec::new()` when empty). Forward `poll_write_vectored` and `is_write_vectored` in `AsyncWrite for PrefixedStream<S>`. Add a unit test verifying partial reads + `buffered()` + `into_inner()` on `PrefixedStream` — done when: `cargo test --locked prefixed` passes.
4. [done] Forward `poll_write_vectored` and `is_write_vectored` in `AsyncWrite for UpstreamStream` (`src/upstream/mod.rs`) and `AsyncWrite for CountingStream<S>` (`src/http/tunnel.rs`, adding `n` to the byte counter on `Poll::Ready(Ok(n))` just like `poll_write`) — done when: `grep -c 'poll_write_vectored' src/net.rs src/upstream/mod.rs src/http/tunnel.rs` shows at least 1 in each file.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
```

## Escalate when
- `connect_happy_eyeballs` triggers a clippy warning or lifetime issue with `FuturesUnordered` (note: `futures` crate is NOT in `Cargo.toml`; use `tokio::task::JoinSet` or `Vec<Pin<Box<...>>>` with `std::future::poll_fn`) → use `tokio::task::JoinSet` or `poll_fn` without adding any crate dependency; if a dependency would be needed, stop, set Status: needs-human — reason: explain why.
- Any existing `src/net.rs` filter or timeout test regresses → stop, set Status: needs-human — reason: paste the failing test output.

## Log
- (append-only, one line per event)
- sub-task 1: added HAPPY_EYEBALLS_DELAY, interleave_addrs, connect_happy_eyeballs (tokio::task::JoinSet, no new deps), shared resolve_within helper; used for filtered multi-candidate dials and unfiltered DNS-name dials (IP literals keep a single connect); added 4 unit tests
- sub-task 2: relay_bidirectional now uses copy_bidirectional_with_sizes with RELAY_BUFFER_SIZE = 32 KiB per direction
- sub-task 3: PrefixedStream gained a `pos` cursor (put_slice, no initialize_unfilled / Vec::drain), buffered()/into_inner() honor the cursor, AsyncWrite forwards poll_write_vectored/is_write_vectored; added prefixed_stream_partial_reads_track_cursor test
- sub-task 4: UpstreamStream and CountingStream forward poll_write_vectored/is_write_vectored (CountingStream counts vectored bytes)
- Verify passed on first run; no fix-ups needed

## Evidence
- (append-only: command → observed result)
- `cargo test --locked happy_eyeballs` → 2 passed (happy_eyeballs_skips_refused_candidate_quickly, happy_eyeballs_reports_last_error_when_all_fail)
- `cargo test --locked interleave_addrs` → 2 passed (interleave_addrs_alternates_families_starting_with_first, interleave_addrs_handles_empty_and_single_family)
- `cargo test --locked net::` → 12 passed, 0 failed (no regressions in existing filter/timeout tests)
- `grep -c 'copy_bidirectional_with_sizes' src/net.rs` → 1
- `cargo test --locked prefixed` → 2 passed (prefixed_stream_partial_reads_track_cursor, prefixed_stream_replays_buffer_before_inner)
- `grep -c 'poll_write_vectored' src/net.rs src/upstream/mod.rs src/http/tunnel.rs` → src/net.rs:2, src/upstream/mod.rs:5, src/http/tunnel.rs:2
- Verify `cargo fmt --check` → exit 0
- Verify `cargo clippy --all-targets --locked -- -D warnings` → Finished, no warnings
- Verify `cargo clippy --all-targets --no-default-features --locked -- -D warnings` → Finished, no warnings
- Verify `cargo test --locked` → 208 passed, 0 failed (lib 190 + integration 5+4+6+2+1)
- Verify `cargo test --no-default-features --locked` → 196 passed, 0 failed (lib 179 + integration 5+4+6+2)
