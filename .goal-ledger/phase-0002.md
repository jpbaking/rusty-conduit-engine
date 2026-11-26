# phase-0002 — Record the change in the divergence ledger and DOX

- Status: done
- Owner: executor
- Depends on: phase-0001
- Goal: Record the backlog change as a divergence-ledger entry and in the DOX Feature Map, as the engine's DOX contract requires.
- Done when: every command in `## Verify` exits 0.
- Pattern to follow: `docs/DIVERGENCE-LEDGER.md` section `## E4 — Port \`rusty-conduit-ssh\`` (table header and row format)

## Context
The engine's `DOX.md` requires an entry in `docs/DIVERGENCE-LEDGER.md` for any change to behaviour a consumer relied on. Every consumer (`rusty-conduit`, `rusty-conduit-ssh`, `rusty-conduit-switch`, `rusty-conduit-gui`) binds through `Server::bind`, so all of them get the new backlog when they move to the next engine tag. The decision is `keep`: it becomes engine behaviour for everyone, with no option. Do not bump the version in `Cargo.toml` and do not edit `CHANGELOG`-style files; the release is a separate step.

## Scope
- In: `docs/DIVERGENCE-LEDGER.md`, `DOX.md`
- Out: `src/`, `tests/`, `Cargo.toml`, `Cargo.lock`, `docs/MASTER-PLAN.md`, every other file

## Sub-tasks
1. [done] Append to the end of `docs/DIVERGENCE-LEDGER.md` a section `## M — Maintenance after the CLI ports` with one sentence of intro (`Engine changes made after E5 that change behaviour every consumer inherits.`), the same six-column table header as the E4 section, and one row: ID `M-01`, Module `` `server` ``, Consumers `all`, Difference `` Listeners are now bound through `bind_listener`, which requests a listen backlog of `LISTEN_BACKLOG` (4096) instead of the 128 that `TcpListener::bind` requests. The kernel still caps it (`net.core.somaxconn`, 4096 by default on Linux ≥ 5.4; `kern.ipc.somaxconn`, 128 by default on macOS). `SO_REUSEADDR` stays on for non-Windows platforms. Connection bursts larger than 128 no longer overflow the queue on Linux. ``, Decision `keep`, Landed `` `src/server.rs` (`bind_listener`, `backlog_queues_more_than_128_connections`) `` — done when: `grep -n '^| M-01 |' docs/DIVERGENCE-LEDGER.md` prints one line.
2. [done] In `DOX.md`, edit only the `## Feature Map` bullet **Listeners and graceful shutdown**: after `connection limits,` insert `a 4096 listen backlog (\`LISTEN_BACKLOG\`, capped by the kernel's somaxconn),` — done when: `grep -n 'LISTEN_BACKLOG' DOX.md` prints one line.

## Verify
```sh
grep -n '^## M — Maintenance after the CLI ports' docs/DIVERGENCE-LEDGER.md
grep -n '^| M-01 |' docs/DIVERGENCE-LEDGER.md
grep -n 'LISTEN_BACKLOG' DOX.md
git diff --exit-code -- src tests Cargo.toml Cargo.lock
cargo test --locked
```

## Escalate when
- The ledger's existing table format differs from six columns → stop, set Status: needs-human — reason: paste the header line.
- The Feature Map bullet text differs from `separate or single-port listeners, connection dispatch, connection limits, drain-on-shutdown` → stop, set Status: needs-human — reason: paste the current bullet.

## Log
- (append-only, one line per event)
- sub-task 1: appended section `## M — Maintenance after the CLI ports` and row `M-01` to `docs/DIVERGENCE-LEDGER.md`
- sub-task 2: updated `## Feature Map` bullet **Listeners and graceful shutdown** in `DOX.md`
- verify: ran all `## Verify` commands; all passed

## Evidence
- (append-only: command → observed result)
- `grep -n '^| M-01 |' docs/DIVERGENCE-LEDGER.md` → exit 0 (1 line at line 79)
- `grep -n 'LISTEN_BACKLOG' DOX.md` → exit 0 (1 line at line 244)
- `grep -n '^## M — Maintenance after the CLI ports' docs/DIVERGENCE-LEDGER.md && grep -n '^| M-01 |' docs/DIVERGENCE-LEDGER.md && grep -n 'LISTEN_BACKLOG' DOX.md && git diff --exit-code -- src tests Cargo.toml Cargo.lock && cargo test --locked` → exit 0 (line 73 heading, line 79 M-01 row, line 244 DOX bullet, no diff in src/tests/Cargo.*, 164 unit + 4 integration tests passed)
