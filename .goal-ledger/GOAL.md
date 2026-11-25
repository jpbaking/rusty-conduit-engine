# GOAL — Complete Milestone E5 CLI Releases

## Goal
- Goal ID: 20261126-e5-cli-releases
- Outcome: Finish tagging, verifying Release workflows (including multi-arch GHCR Docker images), and publishing meaningful divergence-ledger release notes for `rusty-conduit-switch` (`v0.0.1-beta.2`), `rusty-conduit` (`v1.3.0`), and `rusty-conduit-ssh` (`v1.4.0`).
- Done when: All three CLI Release workflows succeed on GitHub Actions and all three GitHub Releases have updated release notes containing their `docs/DIVERGENCE-LEDGER.md` entries.
- Goal status: in-review
- Goal status meaning: drafting | approved | executing | blocked-on-human | in-review | awaiting-acceptance | completed | abandoned
- Last completed phase: phase-0002

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
- [done] phase-0001 — Confirm rusty-conduit-switch CI, tag v0.0.1-beta.2, and watch all 3 Release workflows
- [done] phase-0002 — Update GitHub Release notes for all 3 CLIs with divergence ledger entries

## Handoff
- Current position: all phases done; entering Gate D planner review
- Next role: planner
- Next action: run Gate D review (Full verification, `git diff <baseline>..HEAD`, scope check, evidence check, validator) and transition to `awaiting-acceptance`
- Last verified evidence: phase-0002 `## Verify` passed (all 3 GitHub Releases updated with `rusty-conduit-engine v0.2.0` & divergence ledger entries); note `origin/main` advanced to `3b55f18` (automated squash prohibited on `current-branch`)
- Blockers: none

## Review
- Reviewed by: none
- Verification result: none
- Diff reviewed: none
- Findings: none

## Log
- created E5 handoff ledger after completing E0–E4, releasing rusty-conduit-engine v0.2.0, merging all 3 CLIs to main, and tagging rusty-conduit v1.3.0 and rusty-conduit-ssh v1.4.0
- resumed goal as planner: verified `main` at `3b55f18` (clean tree, baseline `bc05012` ancestor, 1 matching Goal-ID commit, validator clean; noted `origin/main` published at `3b55f18` so automated squash is unavailable); beginning `phase-0001`
- closed phase-0001: tagged & pushed `rusty-conduit-switch` `v0.0.1-beta.2` (`8ee5c3948d18`), all 3 CLI Release workflows (`#45701315242`, `#45701367206`, `#45702789898`) verified `success`
- began phase-0002: updating GitHub Release notes for all 3 CLIs with divergence ledger entries
- closed phase-0002: all 3 GitHub Releases updated and verified; entering Gate D (`in-review`)
