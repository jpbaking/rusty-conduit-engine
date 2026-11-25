# GOAL — Complete Milestone E5 CLI Releases

## Goal
- Goal ID: 20261126-e5-cli-releases
- Outcome: Finish tagging, verifying Release workflows (including multi-arch GHCR Docker images), and publishing meaningful divergence-ledger release notes for `rusty-conduit-switch` (`v0.0.1-beta.2`), `rusty-conduit` (`v1.3.0`), and `rusty-conduit-ssh` (`v1.4.0`).
- Done when: All three CLI Release workflows succeed on GitHub Actions and all three GitHub Releases have updated release notes containing their `docs/DIVERGENCE-LEDGER.md` entries.
- Goal status: executing
- Goal status meaning: drafting | approved | executing | blocked-on-human | in-review | awaiting-acceptance | completed | abandoned
- Last completed phase: none

## Execution
- Execution mode: solo
- Planner tier: primary session
- Executor tier: same
- Full verification: gh -R jpbaking/rusty-conduit-switch release view v0.0.1-beta.2 --json tagName,body && gh -R jpbaking/rusty-conduit release view v1.3.0 --json tagName,body && gh -R jpbaking/rusty-conduit-ssh release view v1.4.0 --json tagName,body

## Git
- Repository: yes
- Strategy: current-branch
- Starting branch: main
- Work branch: main
- Baseline commit: bc05012d78a9e037df0d2e7f7c601974825d1482
- Starting upstream at start: origin/main@bc05012d78a9e037df0d2e7f7c601974825d1482
- Work upstream at start: origin/main@bc05012d78a9e037df0d2e7f7c601974825d1482

## Phases
- [todo] phase-0001 — Confirm rusty-conduit-switch CI, tag v0.0.1-beta.2, and watch all 3 Release workflows
- [todo] phase-0002 — Update GitHub Release notes for all 3 CLIs with divergence ledger entries

## Handoff
- Current position: handed off between machines (all 4 repos pushed & synced to origin/main)
- Next role: planner
- Next action: resume with phase-0001 — check rusty-conduit-switch CI (#45700865218), tag/push v0.0.1-beta.2 once green, and verify Release workflows for rusty-conduit v1.3.0 (#45701315242), rusty-conduit-ssh v1.4.0 (#45701367206), and rusty-conduit-switch v0.0.1-beta.2
- Last verified evidence: rusty-conduit-engine v0.2.0 released & CI green (#45700473885); rusty-conduit main CI green (#45700876479) & v1.3.0 pushed; rusty-conduit-ssh main CI green (#45700888746) & v1.4.0 pushed; rusty-conduit-switch main pushed (93398fc) & CI queued (#45700865218)
- Blockers: none

## Review
- Reviewed by: none
- Verification result: none
- Diff reviewed: none
- Findings: none

## Log
- created E5 handoff ledger after completing E0–E4, releasing rusty-conduit-engine v0.2.0, merging all 3 CLIs to main, and tagging rusty-conduit v1.3.0 and rusty-conduit-ssh v1.4.0
