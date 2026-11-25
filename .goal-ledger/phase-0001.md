# phase-0001 — Confirm rusty-conduit-switch CI, tag v0.0.1-beta.2, and watch all 3 Release workflows

- Status: ongoing
- Owner: planner
- Depends on: none
- Goal: Wait for `rusty-conduit-switch` `main` CI (`#45700865218`), tag and push `v0.0.1-beta.2`, and verify all three CLI Release workflows complete cleanly.
- Done when: `rusty-conduit-switch` `v0.0.1-beta.2`, `rusty-conduit` `v1.3.0` (`#45701315242`), and `rusty-conduit-ssh` `v1.4.0` (`#45701367206`) Release workflows all have conclusion `success`.
- Pattern to follow: none

## Context
All four sibling repositories (`rusty-conduit-engine`, `rusty-conduit-switch`, `rusty-conduit`, `rusty-conduit-ssh`) are already pushed and synced with `origin/main`:
- `rusty-conduit-engine`: `v0.2.0` released (`bc05012`).
- `rusty-conduit`: `main` (`f49f1fc`) CI passed (`#45700876479`); `v1.3.0` tagged & pushed; Release workflow `#45701315242` already running.
- `rusty-conduit-ssh`: `main` (`cc372c9`) CI passed (`#45700888746`); `v1.4.0` tagged & pushed; Release workflow `#45701367206` already running.
- `rusty-conduit-switch`: `main` (`93398fc`) pushed; CI `#45700865218` was queued when handing off; `v0.0.1-beta.2` has NOT been tagged yet.

## Scope
- In: `.goal-ledger/**`, git tag `v0.0.1-beta.2` in `../rusty-conduit-switch`
- Out: application source code in any repository (unless a CI/Release failure requires a fix)

## Sub-tasks
1. [todo] Pull latest `main` in all 4 sibling repos and confirm `rusty-conduit-switch` CI `#45700865218` finished with `success` — done when: `gh -R jpbaking/rusty-conduit-switch run view 45700865218 --json conclusion -q .conclusion` prints `success`
2. [todo] Create annotated tag `v0.0.1-beta.2` on `../rusty-conduit-switch` `main` (`93398fc`) and push `origin v0.0.1-beta.2` — done when: `git -C ../rusty-conduit-switch ls-remote --tags origin v0.0.1-beta.2` is non-empty
3. [todo] Wait for and confirm `success` on all three Release workflows (`rusty-conduit` `#45701315242`, `rusty-conduit-ssh` `#45701367206`, and the newly triggered `rusty-conduit-switch` `v0.0.1-beta.2` Release run) — done when: all three Release runs report `completed/success`

## Verify
```sh
gh -R jpbaking/rusty-conduit run view 45701315242 --json conclusion -q .conclusion | grep -qx success && gh -R jpbaking/rusty-conduit-ssh run view 45701367206 --json conclusion -q .conclusion | grep -qx success && gh -R jpbaking/rusty-conduit-switch run list --workflow=release.yml --limit 1 --json headBranch,conclusion -q '.[0] | "\(.headBranch) \(.conclusion)"' | grep -qx "v0.0.1-beta.2 success"
```

## Escalate when
- Any CI or Release workflow fails → stop, inspect failed job logs, and report exact failure before fixing

## Log
- (append-only, one line per event)
- began phase-0001 on resume

## Evidence
- (append-only: command → observed result)
