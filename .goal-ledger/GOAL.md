# GOAL — Raise the engine listen backlog to 4096

## Goal
- Goal ID: 20261126-listen-backlog
- Outcome: Every engine listener requests a listen backlog of 4096 instead of the 128 that `tokio::net::TcpListener::bind` (via mio/std) hard-codes, so the kernel's own cap (`net.core.somaxconn`, `kern.ipc.somaxconn`) decides the queue length, and the change is recorded in the divergence ledger.
- Done when: the six CI checks in `## Execution` pass; `src/server.rs` binds through a helper that keeps `SO_REUSEADDR` on non-Windows and listens with `LISTEN_BACKLOG = 4096`; on Linux with `somaxconn >= 300` the new test queues all 300 connections before the first accept (the old backlog stops at 129, because Linux queues backlog + 1); `docs/DIVERGENCE-LEDGER.md` has the entry; no version bump and no tag (the release is a separate step the user approves).
- Goal status: completed
- Goal status meaning: drafting | approved | executing | blocked-on-human | in-review | awaiting-acceptance | completed | abandoned
- Last completed phase: phase-0002

## Execution
- Execution mode: delegated
- Planner tier: Claude Code (claude-opus-5-5)
- Executor tier: agy --model gemini-4-neon-medium via the herdr-subagent skill
- Full verification: cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo clippy --all-targets --no-default-features --locked -- -D warnings && cargo test --locked && cargo test --no-default-features --locked && python3 scripts/check-pure-rust-crypto.py

## Git
- Repository: yes
- Strategy: isolated-branch
- Starting branch: main
- Work branch: goal/20261126-listen-backlog
- Baseline commit: 1b67956fe540c920a985a0b311b77e0b1c5629ed
- Starting upstream at start: origin/main@1b67956fe540c920a985a0b311b77e0b1c5629ed
- Work upstream at start: none

## Phases
- [done] phase-0001 — Bind listeners with a 4096 backlog
- [done] phase-0002 — Record the change in the divergence ledger and DOX

## Handoff
- Current position: completed
- Next role: planner
- Next action: none
- Last verified evidence: planner Gate D full verification on 484799e passed (fmt, clippy x2, tests 164 + 153 unit, PURE_RUST_CRYPTO_OK); phase-0002 executor: grep M section & M-01 in docs/DIVERGENCE-LEDGER.md, grep LISTEN_BACKLOG in DOX.md, git diff --exit-code -- src tests Cargo.toml Cargo.lock, cargo test --locked (164 unit + 4 integration) all passed
- Blockers: none

## Review
- Reviewed by: planner (claude-opus-5-5)
- Verification result: full verification passed on macOS arm64; the Linux-only backlog test compiles out here and first runs in CI
- Diff reviewed: 1b67956..484799e (src/server.rs, DOX.md, docs/DIVERGENCE-LEDGER.md, ledger)
- Findings: one, fixed in 6dbd36f: the backlog test's `> 128` bound also passed with the old backlog (Linux queues backlog + 1), so it now requires all 300; no version bump or tag in range

## Log
- created ledger with 2 phases; replaces the completed ledger 20261126-e5-cli-releases (kept in Git history)
- user decision (CLI consistency goal): listen-backlog/somaxconn is its own engine goal; no releases yet
- planner prototype (tokio 1.54.1, socket2 0.6): TcpSocket + set_reuseaddr + listen(4096) binds v4 and v6, keeps SO_REUSEADDR, still reports AddrInUse for a taken port; on macOS (kern.ipc.somaxconn=128) exactly 128 connections queue before accept
- follow-up after this goal (separate, needs the user's go-ahead): `chore: release v0.4.0` + tag, then bump the pin in rusty-conduit, rusty-conduit-ssh, rusty-conduit-switch, and rusty-conduit-gui and document somaxconn tuning in their sizing guides
- user approved the goal, the branch, and execution ("approved, create the branch, and go"); created goal/20261126-listen-backlog from main
- delegated phase-0001 to the executor
- phase-0001 completed by executor: bound listeners with LISTEN_BACKLOG = 4096 and added unit tests; full verification passed
- planner review of phase-0001: accepted; tightened backlog_queues_more_than_128_connections from `> 128` to `== 300`, since Linux accepts backlog + 1 and the old code would have reached 129
- delegated phase-0002 to the executor
- phase-0002 completed by executor: recorded M-01 in docs/DIVERGENCE-LEDGER.md and updated DOX.md Feature Map; verification passed
- Gate D passed; goal awaiting the user's acceptance
- user accepted ("accept, squash, and push engine main"); squashed onto main as one commit; the per-phase history stays on goal/20261126-listen-backlog
