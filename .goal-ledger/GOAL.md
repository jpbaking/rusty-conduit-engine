# GOAL — Optimization roadmap Phase 2 for rusty-conduit-engine

## Goal
- Goal ID: 20261128-engine-optimization-phase2
- Outcome: `rusty-conduit-engine` fixes MITM bypass domain normalization, HTTP/1.1 slowloris header timeouts, MITM H2 stream caps, and `drain_timeout` task aborts (`B-01`, `B-06`), and lands `TCP_NODELAY`, async DNS routing, MITM TLS session/ALPN reuse, Happy Eyeballs dialing, passive upstream failover cooldown, fast-path logging, and data-plane/`CertCache` optimizations (`P-03`–`P-08`, `P-10`), with full Divergence Ledger and DOX updates.
- Done when: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo clippy --all-targets --no-default-features --locked -- -D warnings && cargo test --locked && cargo test --no-default-features --locked && python3 scripts/check-pure-rust-crypto.py` exits 0 and `docs/DIVERGENCE-LEDGER.md` and `DOX.md` document every observable change.
- Goal status: completed
- Goal status meaning: drafting | approved | executing | blocked-on-human | in-review | awaiting-acceptance | completed | abandoned
- Last completed phase: phase-0006

## Execution
- Execution mode: delegated
- Planner tier: Antigravity CLI (Gemini 4 Neon Medium)
- Executor tier: claude --model opus --effort medium via the herdr-subagent skill
- Full verification: cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo clippy --all-targets --no-default-features --locked -- -D warnings && cargo test --locked && cargo test --no-default-features --locked && python3 scripts/check-pure-rust-crypto.py

## Git
- Repository: yes
- Strategy: isolated-branch
- Starting branch: main
- Work branch: goal/20261128-engine-optimization-phase2
- Baseline commit: 2008ee84a5794009c4d73936cfbab7c545feb60b
- Starting upstream at start: origin/main@2008ee84a5794009c4d73936cfbab7c545feb60b
- Work upstream at start: none

## Phases
- [done] phase-0001 — MITM bypass normalization, H2 stream caps/adaptive window, TLS session reuse, and host-scoped URL-rule check
- [done] phase-0002 — Server TCP_NODELAY, HTTP/1.1 header-read timeout, and drain_timeout task abort
- [done] phase-0003 — Happy Eyeballs dialing, 32 KiB relay, vectored writes, and PrefixedStream cursor
- [done] phase-0004 — Async DNS resolution for resolve_dns routing rules and O(1) CertCache eviction
- [done] phase-0005 — Passive upstream failover cooldown and single lazy-timeout dial wrapper
- [done] phase-0006 — Fast-path logging, non-blocking AccessGuard::drop, single-buffer log encoding, and DOX/Divergence Ledger closeout

## Handoff
- Current position: completed
- Next role: planner
- Next action: none
- Last verified evidence: Full verification exited 0 (fmt, clippy default + no-default-features, cargo test 223 passed / no-default-features 211 passed, PURE_RUST_CRYPTO_OK)
- Blockers: none

## Review
- Reviewed by: Antigravity CLI (Gemini 4 Neon Medium), planner
- Verification result: pass — cargo fmt --check, cargo clippy (default + --no-default-features, -D warnings), cargo test --locked (223 passed, 0 failed), cargo test --no-default-features --locked (211 passed, 0 failed), and python3 scripts/check-pure-rust-crypto.py (PURE_RUST_CRYPTO_OK) all exited 0
- Diff reviewed: 2008ee84a5794009c4d73936cfbab7c545feb60b..c974b4e (29 files incl. ledger; zero scope drift; M-08..M-15 in docs/DIVERGENCE-LEDGER.md and DOX closeout verified)
- Findings: none (branch is unpublished and clean; optional squash on acceptance is eligible)

## Log
- created ledger with 6 phases (replaces completed 20261126-listen-backlog ledger upon Gate A approval)
- source plan: ../docs/OPTIMIZATION.md, roadmap Phase 2 (items B-01, B-06, P-03, P-04, P-05, P-06, P-07, P-08, P-10)
- invariants preserved: zero hidden data/DNS/connection caches (CertCache remains sole cache), pure-Rust crypto only, --no-default-features must build and pass tests
- Gate A/B/C approved by user on 2026-11-28; created goal/20261128-engine-optimization-phase2 from main at 2008ee84a5794009c4d73936cfbab7c545feb60b
- delegated phase-0001 to claude --model opus --effort medium via herdr-subagent
- phase-0001 done by executor (claude opus): B-01 bypass normalization, H2 stream cap/adaptive window, shared TLS session storage/ticketer + UpstreamTlsPool, host-scoped URL-rule check; Verify passed
- planner verified phase-0001; delegated phase-0002 to claude --model opus --effort medium via herdr-subagent
- phase-0002 done by executor (claude opus): server TCP_NODELAY, HTTP/1.1 header-read timeout (30s default, `with_header_read_timeout`), drain_timeout aborts remaining connection tasks; Verify passed after one fmt fix-up
- planner verified phase-0002; delegated phase-0003 to claude --model opus --effort medium via herdr-subagent
- phase-0003 done by executor (claude opus): Happy Eyeballs dialing via JoinSet (no new deps), 32 KiB relay buffers, vectored-write forwarding on PrefixedStream/UpstreamStream/CountingStream, PrefixedStream read cursor; Verify passed
- planner verified phase-0003; delegated phase-0004 to claude --model opus --effort medium via herdr-subagent
- phase-0004 done by executor (claude opus): Router::needs_dns/route_with_ips, RouteResolver::resolve_with_ips + resolve_route_async (tokio lookup_host bounded by connect_timeout, no cache) wired into HTTP/MITM/SOCKS, CertCache FIFO O(1) eviction; Verify passed
- planner verified phase-0004; delegated phase-0005 to claude --model opus --effort medium via herdr-subagent
- phase-0005 done by executor (claude opus): lazy `with_timeout` error context, passive per-profile failover cooldown (DEFAULT_FAILOVER_COOLDOWN 3 s, `ProfileRegistry::with_failover_cooldown`, CAS probe claim, clear on success); Verify passed
- planner verified phase-0005; delegated phase-0006 to claude --model opus --effort medium via herdr-subagent
- phase-0006 done by executor (claude opus): log_*! macros + Logger::log_fmt on hot paths, single-buffer visitor encoder (byte-identical), AccessLogger::try_log_http + non-blocking AccessGuard::drop, record skipped when access logging off (HTTP, MITM, SOCKS), Divergence Ledger M-08..M-15 and DOX closeout; Verify passed
- planner Gate D review passed: Full verification exited 0 (223 default tests, 211 --no-default-features tests, PURE_RUST_CRYPTO_OK), 29 files reviewed with zero scope drift; set Goal status to awaiting-acceptance
- user accepted goal on 2026-11-28 and chose squash + merge into main; soft-reset to baseline 2008ee84a5794009c4d73936cfbab7c545feb60b, created single squashed commit retaining .goal-ledger/, and fast-forwarded main
