# phase-0002 — Update GitHub Release notes for all 3 CLIs with divergence ledger entries

- Status: done
- Owner: planner
- Depends on: phase-0001
- Goal: Edit the auto-generated GitHub Release notes for `rusty-conduit-switch` (`v0.0.1-beta.2`), `rusty-conduit` (`v1.3.0`), and `rusty-conduit-ssh` (`v1.4.0`) to include clear summaries of the `rusty-conduit-engine v0.2.0` migration and each CLI's entries from `docs/DIVERGENCE-LEDGER.md`.
- Done when: All three GitHub Releases contain meaningful release notes referencing `rusty-conduit-engine v0.2.0` and their relevant `docs/DIVERGENCE-LEDGER.md` fixes/notes.
- Pattern to follow: `docs/DIVERGENCE-LEDGER.md:22-80`

## Context
Each CLI's `release.yml` creates a GitHub Release with `--generate-notes` once binaries are built. Master Plan §6 E5 requires publishing each CLI's divergence ledger entries in its release changelog:
- `rusty-conduit-switch` `v0.0.1-beta.2`: E1 extraction onto `rusty-conduit-engine v0.2.0`, E2 MITM CA loader fixes (LibreSSL explicit P-256/P-384 EC params + PBES2 HMAC-SHA1 hint), `RoutingCounters` preserved across hot-reload, zero user-visible CLI/routing change.
- `rusty-conduit` `v1.3.0`: E3 migration onto `rusty-conduit-engine v0.2.0` (`default-features = false`), E2 MITM CA loader fixes, unchanged v1.2.0 access-log schema & DNS+IP allowlist behavior, unified `400` bad-CONNECT message and diagnostic wording.
- `rusty-conduit-ssh` `v1.4.0`: E4 migration onto `rusty-conduit-engine v0.2.0` (`default-features = false`), E2 MITM CA loader fixes, `Socks5UpstreamDialer` wired as external profile + `ReverseProxy` on engine `Server`, unchanged v1.3.0 `direction=forward|reverse` access-log schema.

## Scope
- In: `.goal-ledger/**`, GitHub Releases via `gh release edit` on `jpbaking/rusty-conduit-switch`, `jpbaking/rusty-conduit`, and `jpbaking/rusty-conduit-ssh`
- Out: application source code or git tags

## Sub-tasks
1. [done] Update `jpbaking/rusty-conduit-switch` release `v0.0.1-beta.2` notes via `gh release edit` — done when: `gh -R jpbaking/rusty-conduit-switch release view v0.0.1-beta.2 --json body -q .body | grep -q "rusty-conduit-engine"`
2. [done] Update `jpbaking/rusty-conduit` release `v1.3.0` notes via `gh release edit` — done when: `gh -R jpbaking/rusty-conduit release view v1.3.0 --json body -q .body | grep -q "rusty-conduit-engine"`
3. [done] Update `jpbaking/rusty-conduit-ssh` release `v1.4.0` notes via `gh release edit` — done when: `gh -R jpbaking/rusty-conduit-ssh release view v1.4.0 --json body -q .body | grep -q "rusty-conduit-engine"`

## Verify
```sh
gh -R jpbaking/rusty-conduit-switch release view v0.0.1-beta.2 --json body -q .body | grep -q "rusty-conduit-engine" && gh -R jpbaking/rusty-conduit release view v1.3.0 --json body -q .body | grep -q "rusty-conduit-engine" && gh -R jpbaking/rusty-conduit-ssh release view v1.4.0 --json body -q .body | grep -q "rusty-conduit-engine"
```

## Escalate when
- Any of the three GitHub Releases does not exist yet → stop, check the corresponding Release workflow

## Log
- (append-only, one line per event)
- began phase-0002
- sub-tasks 1–3 done & phase verified: updated GitHub Release notes for `rusty-conduit-switch` `v0.0.1-beta.2`, `rusty-conduit` `v1.3.0`, and `rusty-conduit-ssh` `v1.4.0` with `rusty-conduit-engine v0.2.0` migration summaries and `docs/DIVERGENCE-LEDGER.md` entries; `## Verify` passed

## Evidence
- (append-only: command → observed result)
- `gh -R jpbaking/rusty-conduit-switch release edit v0.0.1-beta.2 --notes-file -` → `https://github.com/jpbaking/rusty-conduit-switch/releases/tag/v0.0.1-beta.2`
- `gh -R jpbaking/rusty-conduit release edit v1.3.0 --notes-file -` → `https://github.com/jpbaking/rusty-conduit/releases/tag/v1.3.0`
- `gh -R jpbaking/rusty-conduit-ssh release edit v1.4.0 --notes-file -` → `https://github.com/jpbaking/rusty-conduit-ssh/releases/tag/v1.4.0`
- `## Verify` (`gh release view ... | grep -q "rusty-conduit-engine"` across all 3 repos) → exit 0 (`PHASE_0002_VERIFY=PASS`)
