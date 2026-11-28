# phase-0006 — Fast-path logging, non-blocking AccessGuard::drop, single-buffer log encoding, and DOX/Divergence Ledger closeout

- Status: done
- Owner: executor
- Depends on: phase-0005
- Goal: Eliminate hot-path allocations when logging is disabled (`P-08`), use non-blocking `try_send` in `AccessGuard::drop` (`P-08`), encode logfmt/JSON directly into a single pre-sized buffer (`P-08`), and record all Phase 2 changes in `docs/DIVERGENCE-LEDGER.md` and `DOX.md` (plus affected child `DOX.md`s).
- Done when: every command in `## Verify` exits 0.
- Pattern to follow: `docs/DIVERGENCE-LEDGER.md:78-84` (entries `M-01`–`M-07`) and `src/logging.rs:175-210, 460-518`

## Context
At default `INFO` log level, hot-path `logger.debug(format!(...))` and `logger.verbose(format!(...))` calls still allocate formatted `String`s before `Logger::log` discards them.
`RequestCtx::access_guard` allocates an `HttpAccessRecord` and calls `OffsetDateTime::now_utc()` even when `!access.http_enabled()`, and `AccessGuard::drop` spawns a `tokio::spawn` task on every HTTP request even when the 4,096-slot mpsc queue has room.
`encode` in `src/logging.rs` allocates an intermediate `Vec<(&str, Value)>` plus per-field `String`s on every logged record.
Finally, `DOX.md` requires recording every observable engine behavior change in `docs/DIVERGENCE-LEDGER.md` and refreshing affected `DOX.md` docs.

## Scope
- In: `src/logging.rs`, `src/http/mod.rs`, `src/http/forward.rs`, `src/http/tunnel.rs`, `src/http/mitm_tunnel.rs`, `src/socks/mod.rs`, `src/server.rs`, `src/upstream/mod.rs`, `docs/DIVERGENCE-LEDGER.md`, `DOX.md`, `src/http/DOX.md`, `src/mitm/DOX.md`, `src/router/DOX.md`, `src/upstream/DOX.md`
- Out: `Cargo.toml`, `Cargo.lock`, `scripts/`, `.github/`

## Sub-tasks
1. [done] In `src/logging.rs`, add `pub fn log_fmt(&self, level: LogLevel, args: fmt::Arguments<'_>)` on `Logger` and `pub(crate)` macros `log_error!`, `log_info!`, `log_debug!`, `log_verbose!` that check `$logger.enabled(...)` before calling `format_args!(...)`. Replace hot-path `logger.debug(format!(...))` and `logger.verbose(format!(...))` calls in `src/server.rs`, `src/http/`, `src/socks/mod.rs`, and `src/upstream/mod.rs` with `log_debug!` / `log_verbose!` (keep the existing `Logger::debug`/`verbose` public methods intact) — done when: `cargo test --locked logging` passes.
2. [done] In `src/logging.rs`, rewrite `HttpAccessRecord::format_with` and `SocksLogRecord::format_with` to encode logfmt directly into `String::with_capacity(256)` via `std::fmt::Write` (no intermediate `Vec<(&str, Value)>` and no per-field `String` allocations; write `logfmt_value` directly into the target `&mut String`), and encode JSON directly via a zero-vec `Serialize` wrapper on the record structs — preserving byte-identical text and JSON output (every existing `src/logging.rs` formatting test must pass unchanged) — done when: `cargo test --locked logging` passes.
3. [done] In `src/logging.rs`, add a non-blocking `pub(crate) fn try_log_http(&self, record: &HttpAccessRecord) -> Result<(), String>` (or `Option<String>` returning the formatted line only if `tx.try_send` returned `TrySendError::Full`, after invoking `sink` if present). In `src/http/mod.rs`, skip `OffsetDateTime::now_utc()` and `HttpAccessRecord` allocation in `RequestCtx::access_guard` when `!self.proxy.access.http_enabled()` (setting `record: None` while keeping `bytes_in`/`bytes_out`/`traffic` intact), and in `AccessGuard::drop` call `try_log_http` first, spawning onto `self.tasks` only when the channel is full. In `src/socks/mod.rs`, skip constructing `SocksLogRecord` and calling `OffsetDateTime::now_utc()` when `!self.access.socks_enabled()` — done when: `cargo test --locked` passes.
4. [done] Record all Phase 2 observable changes in `docs/DIVERGENCE-LEDGER.md` (continuing from `M-07` with `M-08` onward: MITM bypass normalization, HTTP/1.1 header-read timeout + MITM H2 stream cap + `drain_timeout` task abort, `TCP_NODELAY`, async DNS routing, MITM TLS session reuse + scoped URL rules, Happy Eyeballs, passive failover cooldown, 32 KiB relay / `CertCache` O(1) eviction / fast-path logging). Do a DOX closeout pass updating `DOX.md` and any affected child `DOX.md`s (`src/http/DOX.md`, `src/mitm/DOX.md`, `src/router/DOX.md`, `src/upstream/DOX.md`) where local contracts or Feature Maps changed — done when: `grep -c 'M-08' docs/DIVERGENCE-LEDGER.md` is at least 1 and all Verify commands pass.

## Verify
```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
python3 scripts/check-pure-rust-crypto.py
```

## Escalate when
- Any existing `src/logging.rs` formatting test changes output even by one character → fix the encoder to match byte-for-byte; if a test depends on internal `Fields` trait visibility, stop, set Status: needs-human — reason: paste the error.
- `scripts/check-pure-rust-crypto.py` fails → stop, set Status: needs-human — reason: paste the output.

## Log
- (append-only, one line per event)
- ST1: added Logger::log_fmt and pub(crate) log_error!/log_info!/log_debug!/log_verbose! (level-checked before format_args!); replaced 42 hot-path debug/verbose(format!(..)) calls in server.rs, http/{mod,forward,tunnel,mitm_tunnel}.rs, socks/mod.rs, upstream/mod.rs
- ST2: replaced Vec<(&str, Value)> + per-field Strings with a FieldVisitor (stack Value incl. Ts/Addr) shared by a TextEncoder writing into String::with_capacity(256) via fmt::Write (write_logfmt_value in place, inline Timestamp Display) and a zero-vec JsonRecord Serialize wrapper streamed by serde_json::to_writer into Vec::with_capacity(256); added inline_timestamp_matches_format_timestamp and logfmt_escapes_control_chars_in_place tests
- ST3: added AccessLogger::try_log_http (sink + try_send; returns the line only on TrySendError::Full) and send_http_line; AccessGuard::drop now calls try_log_http and spawns onto self.tasks only when the queue is full; RequestCtx::access_guard (and the MITM new_guard, same pattern) return AccessGuard::untracked (record: None, counters/traffic intact) when !http_enabled; socks Session drops its eager ts and finish() returns before building SocksLogRecord/reading the clock when !socks_enabled (ts now derived as now_utc - elapsed at emit time, since Session::new callers live in out-of-scope v4.rs/v5.rs); added try_log_http_queues_without_blocking_and_hands_back_on_full test
- ST4: appended M-08..M-15 to docs/DIVERGENCE-LEDGER.md (bypass normalization; header-read timeout + H2 stream cap + drain abort; TCP_NODELAY; async DNS routing; MITM TLS session reuse + scoped URL rules; Happy Eyeballs; failover cooldown; 32 KiB relay / CertCache FIFO / fast-path logging incl. SOCKS ts note); DOX closeout in DOX.md (no-hidden-caches wording now names TLS session-resumption state, new hot-path diagnostics contract, listener/resolver/net/logging Feature Map), src/http, src/mitm, src/router, src/upstream DOX.md
- Verify block passed on first run; no fix-ups; phase closed

## Evidence
- (append-only: command → observed result)
- cargo test --locked logging → 19 passed; 0 failed (lib); grep '\.(debug|verbose)\(' in-scope files → no matches
- cargo test --locked logging → 21 passed; 0 failed (19 pre-existing unchanged + 2 new); git diff src/logging.rs removes no lines inside mod tests
- cargo test --locked → lib 205 passed; integration 5+4+6+2+1 passed; 0 failed
- grep -c 'M-08' docs/DIVERGENCE-LEDGER.md → 1
- Verify: cargo fmt --check → 0; clippy default -D warnings → 0; clippy --no-default-features -D warnings → 0; cargo test --locked → 0 (lib 205 + integration 18 passed); cargo test --no-default-features --locked → 0 (lib 194 + integration 17 passed); python3 scripts/check-pure-rust-crypto.py → 0, PURE_RUST_CRYPTO_OK
