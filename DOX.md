# DOX framework

DOX v6.1.0 — framework source: https://github.com/jpbaking/dox

DOX is a hierarchy of DOX.md files that keeps a project understandable. Each folder's DOX.md is the local contract for everything beneath it; together they form a tree from the repository root down to each work area. Follow DOX across every edit in this project.

## Core Contract

- Each DOX.md is the binding contract for its subtree.
- Any work product, source material, instruction, record, asset, or durable doc must stay understandable from the nearest DOX.md plus every DOX.md above it.
- No child doc may weaken a rule set by a parent doc.

## Hierarchy

- The root DOX.md holds project-wide instructions, global preferences, durable workflow rules, its own Feature Map, and the top-level Child DOX Index. The root's Feature Map and Child DOX Index follow the same rules as any child doc's.
- Each child DOX.md owns the instructions for its own folder and lists its own children.
- The closer a doc is to the work, the more specific and practical it is. Broad rules live in parents; concrete details live in children.
- Each parent explains what its direct children cover and what the parent keeps for itself.

## Where a doc goes: boundaries

A **boundary** is a folder that earns its own DOX.md. Apply this test to every folder, at the root and at every level below it, all the way down. Depth does not matter — a deeply nested folder gets a doc on exactly the same terms as a top-level one.

**Create a DOX.md for a folder when ANY of these is true:**

- It is a submodule, subproject, or nested repository (git submodule, SVN external, Perforce stream or mapped depot path, workspace/monorepo package, or a project you maintain inside this one).
- It is separately built, run, tested, or deployed — it has its own `package.json`, `pyproject.toml`, `go.mod`, `Cargo.toml`, `Makefile`, `Dockerfile`, or similar.
- It has its own purpose or audience that differs from its parent.
- It carries its own contracts, interfaces, or rules that differ from the parent's.

**Do NOT create one when ALL of these are true:**

- It is just a grouping folder (for example `utils/`, `helpers/`, `assets/`) with no distinct contract.
- Everything in it already follows the parent's rules.
- A doc here would only repeat the parent.

When unsure, do not create one — the nearest parent covers it. But **never skip a real submodule or subproject**; those always get a doc.

**Go as deep as the structure goes.** A boundary inside a boundary gets its own child DOX.md. A submodule that itself contains submodules or subprojects gets a full DOX subtree beneath it. There is no depth limit and no preference for a flat tree — the doc tree should mirror the project's real structure.

**Sub-roots.** When a boundary is a self-contained submodule or subproject, treat its DOX.md as a *sub-root*: write it like a root (full local contract, plus its own workflow or verification rules where they differ from the parent) and build a normal DOX tree beneath it. A sub-root still may not weaken any rule from the docs above it.

**Nested roots.** A sub-root that is also the root of its own independently versioned project is a *nested root* — a git submodule or nested git repo, an SVN external, a Perforce stream or mapped depot path, or any folder synced from another repository. The marker is the doc, not the version control system: **any descendant folder whose DOX.md carries the full DOX rules is a nested root. A pre-v3 nested root may instead carry those rules in AGENTS.md; recognize and protect it on the same terms.** It plays two roles at once — root of its own project, sub-root inside this tree — so the same doc works whichever folder an engineer roots their workspace at. Follow these rules exactly:

1. **Leave its doc as it is.** A nested root keeps its full copy of the DOX rules, its root shape, and its current filename. Never rewrite it into Child Doc Shape, rename it from the parent project, or strip its rules — that breaks the project when it is developed standalone.
2. **Read it as a local root.** When walking the doc chain, treat the nested root's DOX.md (or legacy AGENTS.md) as the root for everything beneath it; the parent chain still applies above it.
3. **Never edit it to resolve a conflict.** If a nested root's rule conflicts with a parent rule, do not change the nested root — it is owned by another project. Report the conflict to the user and let them decide.
4. **The parent speaks in its own doc.** Anything the parent expects from the nested project goes in the parent's DOX.md (its Child DOX Index line and, if needed, a Local Contract) — never into the nested root's doc.
5. **Respect the repository boundary.** Any file change inside a nested root — its DOX.md included — belongs to that project's own version history: commit or submit it there, following that system's convention (for a git submodule, the parent repo then updates its submodule pointer). State this in your report whenever you touch files under a nested root.

### Classify AGENTS.md before migration

In v3, an AGENTS.md is not a DOX doc merely because of its filename. Classify it by content and location before any rename or rewrite:

- **Root shim** — points to the sibling DOX.md. Keep it as AGENTS.md; never migrate it.
- **Legacy framework root** — contains `# DOX framework`. At the project root it is the pre-v3 framework doc; below the project root it marks a legacy nested root that the parent project must not touch.
- **Legacy child DOX doc** — follows Child Doc Shape, including `## Child DOX Index`, and has no sibling DOX.md. It may be migrated by the owning project.
- **Unrelated or ambiguous harness file** — preserve it unchanged and report the ambiguity. Never assume it is a legacy DOX doc.

Before any legacy rename, confirm the destination DOX.md does not exist. Never overwrite either file to resolve a collision.

## Child Doc Shape

A child DOX.md uses these sections, in this order. Omit a section if it would be empty — except keep the Child DOX Index and mark it `(none)` at a leaf, so the tree stays explicit.

- **Purpose** — what this folder is for.
- **Ownership** — what this doc governs and what it leaves to parent or child docs.
- **Local Contracts** — rules, interfaces, or constraints specific to this folder. A sub-root puts its build/run/test commands and its own workflow rules here.
- **Work Guidance** — current standards or user instructions for work here. Leave empty if none exist yet.
- **Verification** — how to check work here (tests, lint, build). Leave empty if no such check exists yet.
- **Feature Map** — features implemented in this subtree, each pointing to its entry file and supporting files. Omit if none yet. See the Feature Map section below for the format and rules.
- **Child DOX Index** — one line per direct child DOX.md, naming what it covers. Mark `(none)` at a leaf.

Example child DOX.md (a leaf):

```markdown
# services/auth

## Purpose
Authentication service: login, sessions, and token issuance.

## Ownership
Owns code under services/auth/. Database schema is owned by ../db.

## Local Contracts
- All endpoints return the shared Error shape from ../shared/errors.
- Never log raw tokens.

## Verification
- `npm test` in this folder must pass before any commit.

## Feature Map
- **Login** — email/password sign-in; issues a session. Start: `login.ts`. Files: `session.ts`, `password.ts`.
- **Token issuance** — signs and refreshes access tokens. Start: `tokens.ts`. Files: `keys.ts`.

## Child DOX Index
- (none)
```

Example sub-root (a standalone subproject with its own children):

```markdown
# packages/payments

Sub-root: standalone subproject with its own build and deploy.

## Purpose
Payments service: charges, refunds, and provider integrations.

## Ownership
Owns everything under packages/payments/. Inherits all root DOX rules and adds the local rules below. Does not weaken any parent rule.

## Local Contracts
- Build, test, and run from this folder: `make test`, `make run`.
- All money values use integer minor units. Never use floats.

## Verification
- `make test` must pass before any commit that touches this subtree.

## Feature Map
- **Charge a card** — authorizes and captures a payment. Start: `charge.ts`. Files: `providers/`, `ledger/post.ts`. Detail in ./providers and ./ledger.
- **Refund** — reverses a charge and updates the ledger. Start: `refund.ts`. Files: `ledger/post.ts`.

## Child DOX Index
- providers/ — adapters for each external payment provider.
- ledger/ — double-entry ledger and reconciliation.
```

## Feature Map

The Feature Map answers "what does this part of the system do, and which files do I open to work on it?" It lets an agent start a feature with minimal code traversal, and lets the whole tree be aggregated into an architecture overview.

It is separate from the Child DOX Index on purpose. The Child DOX Index maps **docs to child docs** (how to navigate the doc tree). The Feature Map maps **features to source files** (how to navigate the code). They are different axes — keep them in different sections.

**Format.** Each DOX.md lists the features whose code lives mostly within its own subtree. One bullet per feature:

```
- **<Feature name>** — <one line: what it does>. Start: `<entry file>`. Files: `<other files or folders>`.
```

- **Start** is the single file to open first — the entry point, or the clearest place to begin reading.
- **Files** are the other source files or folders that implement it. Name a folder, not every file inside it, when the whole folder belongs to the feature.

**Where a feature goes (locality).**

- Put a feature in the DOX.md closest to its code.
- If a feature's code spans several folders, put it in the lowest folder whose subtree contains all of it. Name the feature there and point into the child docs that hold each slice (`Detail in ./child`); each child may list its own slice.
- The root DOX.md keeps its own Feature Map under exactly the same rules: it owns the features whose lowest common subtree is the whole project, plus the project's primary, system-wide features. When the detail lives deeper, a root entry points to the owning doc instead of listing files.

**Keep it current.** When a change adds, removes, renames, or relocates a feature — or moves the files behind one — update the Feature Map in the owning doc in the same pass. A stale map is worse than none.

**Architecture overview.** To produce one, walk the DOX tree from the root and collect every Feature Map. Grouped by feature, the result is a feature-to-files map of the whole system — with no separate source of truth to maintain.

## Initialization

Run this when asked to initialize or index the project. Work top-down, then recurse into every boundary.

1. **Map the folder.** List its directory tree. Skip vendored, build, and version-control dirs (`node_modules`, `dist`, `build`, `target`, `.git`, `.svn`, and similar). Write down each folder and a one-line note on its purpose.
2. **Mark the boundaries.** Apply the boundary test (above) to every folder you listed. Mark each one "doc" or "no doc." Always mark submodules and subprojects "doc."
3. **Recurse.** For each folder you marked "doc," repeat steps 1–2 *inside* that folder. Keep going until you reach folders that contain no further boundaries. Do not stop at the top level — go as deep as the structure goes. If any folder already carries its own root DOX.md — or a legacy AGENTS.md — with the full DOX rules, it is a nested root: keep its whole doc tree as it is, index it as a child, and do not rebuild anything inside it.
4. **Write the docs.** Keep the DOX rules in the root DOX.md only — a nested root keeps its own copy; leave it unchanged. Write every other DOX.md using Child Doc Shape above; write a submodule or subproject as a sub-root. Leave Work Guidance and Verification empty where no standard or check exists yet.
5. **Wire the indexes.** In every doc that has children, fill the Child DOX Index — one line per direct child, naming what it covers. Mark a leaf `(none)`.
6. **Map the features.** In every doc — the root DOX.md included — fill the Feature Map with the features you can identify from the code, one bullet per feature, each with its Start file and supporting files. Put each feature in the doc closest to its code (see Feature Map locality); the root's map holds the project-spanning and primary features.
7. **Shim agent harnesses.** Ensure `AGENTS.md` in the project root points to DOX so Antigravity CLI, Codex, Cline, OpenCode, and other compatible harnesses automatically follow it. If missing, create it with: `This project uses the DOX framework. Do not add DOX rules here. Read DOX.md in this directory and follow its instructions.` If it already contains that direction, leave it unchanged. If it contains unrelated harness instructions, prepend the direction once and preserve the existing text. If it contains the full legacy DOX framework, stop and use the upgrade procedure instead; never overwrite it as a shim. Then ensure Claude Code is bridged: if `CLAUDE.md` is missing, create it with `@AGENTS.md`; if it already imports `AGENTS.md` or directly tells Claude to read `DOX.md`, leave it unchanged; otherwise prepend `@AGENTS.md` and preserve its existing instructions. Then ensure Gemini CLI reads `AGENTS.md`, which it does only when `.gemini/settings.json` lists it in `context.fileName`. If `.gemini/settings.json` is missing, create it with `{"context": {"fileName": ["AGENTS.md", "GEMINI.md"]}}`. If it exists, merge and never replace it: if `context.fileName` is absent, set it to `["AGENTS.md", "GEMINI.md"]`; if it is a single string, turn it into a list that keeps that value and adds `"AGENTS.md"`; if it is a list without `"AGENTS.md"`, add it; keep every other setting unchanged. If the file is not plain valid JSON (for example, it contains comments), do not rewrite it; tell the user the exact change to make. Never create a project `GEMINI.md` for DOX.
8. **Report.** Print the full tree you created and name any folder you deliberately left without a doc.

Done when: every boundary at every depth has a DOX.md, every Child DOX Index is filled (or `(none)` at a leaf), each doc's Feature Map — the root's included — lists the features identifiable at its level, the `AGENTS.md`, `CLAUDE.md`, and `.gemini/settings.json` bridges exist, and no index still reads "Not yet indexed" or "Not yet mapped."

## Read Before Editing

Before editing any file:

1. Read the root DOX.md.
2. List the files and folders you expect to touch.
3. For each target, walk from the root down to it, reading every DOX.md along the way — including any sub-root or nested root in the path. During a partial pre-v3 migration, read a legacy child AGENTS.md when no DOX.md exists in that folder. Do not mistake the root AGENTS.md shim for a child doc.
4. Treat the nearest DOX.md as the local contract and the parents as repo-wide rules.
5. If two docs conflict, the closer one controls local details — but no child or sub-root may weaken DOX itself.

Re-read the applicable chain in the current session. Do not rely on memory.

## Update After Editing

Every meaningful change requires a DOX pass before the task is done. Update the closest owning DOX.md when a change affects:

- purpose, scope, ownership, or responsibilities;
- durable structure, contracts, workflows, or operating rules;
- required inputs, outputs, permissions, constraints, side effects, or artifacts;
- the set of features in this subtree, or the files that implement a feature (update the Feature Map in the owning doc);
- user preferences about behavior, communication, process, organization, or quality;
- any DOX.md creation, deletion, move, rename, or index change.

Update a parent when parent-level structure, ownership, workflow, or its child index changes. Update a child when a parent change alters its local rules. Remove stale or contradictory text immediately. A change that alters no behavior or contract may leave docs unchanged — but still do the pass to confirm that.

When the owning doc lies inside a nested root, make the update there — but say in your report that the change belongs to that project: commit or submit it in that project's own repository (for a git submodule, the parent repo then updates its pointer).

## Style

- Keep docs concise, current, and operational. Document stable contracts, not history.
- Prefer direct bullets with explicit names.
- Match the doc tree to the real structure: go deep where the project is deep, and do not flatten genuine boundaries to save docs.
- Do not duplicate a rule across files unless each scope needs its own version.
- Delete stale notes instead of explaining how things used to be.
- Trim obvious statements, repeated rules, misplaced detail, and warnings for risks that no longer exist.

## Closeout

1. Re-check changed paths against the DOX chain.
2. Update the nearest owning docs and any affected parents or children.
3. Refresh every affected Child DOX Index.
4. Refresh every affected Feature Map.
5. Remove stale or contradictory text.
6. Run existing verification when relevant.
7. If any changed file — doc or source — lies inside a nested root, say so: it must be committed or submitted in that project's own repository (for a git submodule, the parent repo also updates its pointer).
8. Report any docs you intentionally left unchanged and why.

## Project Contract

`rusty-conduit-engine` is a library-only Rust crate (`rusty_conduit_engine`, BSD-3-Clause) holding the HTTP/HTTPS and SOCKS proxy engine, routing, upstream dialers, and optional HTTPS MITM shared by the rusty-conduit CLI family (`rusty-conduit`, `rusty-conduit-ssh`, `rusty-conduit-switch`, and `rusty-conduit-gui`). Open items: `docs/TODO.md`.

- **Scope.** This repository only. Consumers live in sibling repositories; never edit them from here. Development happens on `main`.
- **No CLI concerns in default builds.** No argument parsing, config-file loading, file watching, or SSH process supervision anywhere in the crate, and no CLI support at all unless a consumer opts in. Never add `clap`, `notify`, or a non-dev `toml` dependency. The CLIs deserialize their own config formats into the serde types in `src/routing.rs` (which also derive `Serialize`, omitting default-valued fields).
- **Opt-in CLI support (`cli-support` feature, off by default).** Only behind this feature may the crate raise the open-file limit (`rlimit`), check paths before startup (`libc`), wait for shutdown signals (tokio `signal`), and define shared config-file section types (serde only, no file loading). Module: `src/cli_support/`.
- **Opt-in test fixtures (`test-support` feature, off by default).** Helpers for consumers' tests (test CA, echo server, HTTP origin, TLS client, logger capture), built only from existing dependencies. Not covered by semver stability. Module: `src/test_support.rs`.
- **Opt-in `.bak` importer (`import` feature, off by default; enables `routing`).** Converts AirSwitch / ZeroOmega / SwitchyOmega backups into routing config and renders commented TOML by hand (no `toml` dependency). Module: `src/import/`.
- **Pure-Rust crypto.** TLS and certificates use `rustls` with the `ring` provider, plus `rcgen`, `x509-parser`, and `p12-keystore`. `openssl-sys` and `aws-lc-sys` must never enter the normal dependency graph on any target. `scripts/check-pure-rust-crypto.py` enforces this.
- **Releases.** Tag `vX.Y.Z` with an annotated tag whose message body is the release notes (`git tag -a vX.Y.Z -F notes.md`). Pushing the tag runs `.github/workflows/release.yml`, which re-runs verification and creates the GitHub Release from that body. Follow the workspace's engine release order when one applies.
- **`routing` cargo feature (default on).** It gates `regex`. Every module must still build and pass tests with `--no-default-features`. Regex-based conditions then fail to compile with an error that names the `routing` feature.
- **No hidden caches.** Every dial resolves DNS afresh: no DNS cache, no connection pooling (a MITM tunnel holds one upstream connection at a time and redials it when it closes). The MITM leaf-certificate cache (`src/mitm/cache.rs`) is the only data cache in the engine. The only other retained state is rustls TLS session-resumption state for MITM (one bounded server session store and ticketer per `MitmInterceptor`, plus the upstream `ClientConfig` session store); it holds no request or response data. The router's per-minute local-time memo (`SystemClock`, one `AtomicU64`) is not a data cache: it holds the current minute's weekday and minute of day and is recomputed whenever the minute changes, so it is never stale.
- **Cheap diagnostics on hot paths.** Per-connection and per-request diagnostics use the crate's `log_debug!` / `log_verbose!` macros (`src/logging.rs`), which skip formatting when the level is off; never pass `format!(...)` to `Logger::debug`/`verbose` on a hot path.
- **Credentials stay secret.** Credentials, private keys, and passphrases never appear in `Debug` output, logs, or error strings. The `upstream` field of access records is always redacted.
- **Divergence ledger.** Any behavior that differs from a consumer's former copy, or any change to behavior a consumer relied on, gets an entry in `docs/DIVERGENCE-LEDGER.md` with a decision (`keep`, `option`, or `fix`). Never drop a difference silently.
- **Verification.** Before finishing a code change, run the CI checks from `.github/workflows/ci.yml`:
  - `cargo fmt --check`
  - `cargo clippy --all-targets --locked -- -D warnings`
  - `cargo clippy --all-targets --no-default-features --locked -- -D warnings`
  - `cargo test --locked`
  - `cargo test --no-default-features --locked`
  - `cargo clippy --all-targets --all-features --locked -- -D warnings`
  - `cargo test --all-features --locked`
  - `python3 scripts/check-pure-rust-crypto.py` (prints `PURE_RUST_CRYPTO_OK`)

## User Preferences

When the user requests a durable behavior change, record it here or in the relevant child DOX.md.

- (none recorded yet)

## Feature Map

- **Crate entry and module layout** — public module tree and crate docs. Start: `src/lib.rs`. Files: `Cargo.toml`.
- **Listeners and graceful shutdown** — separate or single-port listeners, connection dispatch, connection limits, `TCP_NODELAY` on accepted sockets, a 4096 listen backlog (`LISTEN_BACKLOG`, capped by the kernel's somaxconn), drain-on-shutdown that aborts connection tasks still running when `drain_timeout` expires (`Server`, `EngineHandlers`, `ShutdownReport`, `ConnectionTasks`). Start: `src/server.rs`. Files: `src/mux.rs`.
- **Single-port protocol sniffing** — classifies a connection as HTTP, SOCKS4, or SOCKS5 by peeking its first byte. Start: `src/mux.rs`.
- **HTTP/HTTPS proxy engine** — HTTP forward proxy, opaque `CONNECT` tunnel, the MITM interception path, and HTML error pages for proxy-generated errors. Start: `src/http/mod.rs`. Detail in ./src/http.
- **SOCKS4/4a/5 proxy engine** — TCP `CONNECT` server with the relay-guard hook and SOCKS web blocking. Start: `src/socks/mod.rs`. Detail in ./src/socks.
- **HTTPS MITM** — user-supplied CA loading, per-host leaf signing and caching, TLS interception of `CONNECT`. Start: `src/mitm/mod.rs`. Files: `src/http/mitm_tunnel.rs`, `tests/mitm_counters.rs`, `tests/mitm_h2_upstream_reconnect.rs`. Detail in ./src/mitm and ./src/http.
- **Routing model and hot-reload snapshots** — serde routing/profile types (`Deserialize` and `Serialize`, default-valued fields omitted), `RoutingInput`, compiled immutable `RoutingState`, and the lock-free `SharedRouting` (`arc-swap`) handle the engines read from. Start: `src/routing.rs`. Files: `src/router/`, `src/upstream/`.
- **Rule matching and validation** — the Auto Switch `Router` (host-only rule runs indexed for first-match lookups; exact names, `*.d` / `**.d` suffixes, and combined regexes per condition), rule conditions, and configuration validation. Start: `src/router/mod.rs`. Detail in ./src/router.
- **Upstream dialers and failover** — `direct`, `block`, `http`/`https`, `socks5`, `socks4a`, external dialers, `FailoverDialer`, `ProfileRegistry`. Start: `src/upstream/mod.rs`. Detail in ./src/upstream.
- **Custom route resolver** — the `RouteResolver` trait lets embedders override routing decisions in the HTTP and SOCKS engines; `resolve_route_async` pre-resolves `resolve_dns` hostnames without blocking and passes them through `RouteResolver::resolve_with_ips`. Start: `src/routing.rs`. Files: `src/server.rs`, `tests/route_resolver.rs`.
- **External dialers (API-only)** — embedders register their own dialer (for example an SSH channel) for an `external` profile; never exposed in a config file. Start: `src/upstream/mod.rs`. Files: `src/routing.rs`.
- **Traffic counters** — live per-profile and per-rule `hits`, `active`, `bytes_in`, `bytes_out`. On reload, `RoutingState::with_counters_from` keeps only counters that still describe the new configuration. Start: `src/routing.rs`. Files: `src/http/mod.rs`, `src/http/tunnel.rs`, `src/socks/mod.rs`, `tests/traffic_counters.rs`.
- **Destination filter** — allow/block lists for IP, CIDR, and FQDN (with `*.` wildcards), enforced on both names and resolved IPs. The router checks it through `CompiledFilter` (domain rules hashed). Start: `src/filter.rs`. Files: `src/net.rs`, `src/router/mod.rs`.
- **TCP dial and relay helpers** — OS-resolver dialing with timeouts and RFC 8305 Happy Eyeballs for multi-address hosts, TCP keepalive, `PrefixedStream` (read cursor, vectored writes), bidirectional relay with 32 KiB buffers. Start: `src/net.rs`.
- **Logging and access records** — 6-level diagnostic logger with level-checked `log_*!` macros, text/JSON access logs encoded into one buffer per line (file sinks flushed whenever the writer queue drains, non-blocking `try_log_http` for `AccessGuard::drop`), and the structured `AccessRecord` sink for embedders. Start: `src/logging.rs`. Files: `src/http/mod.rs` (`AccessGuard`), `src/socks/mod.rs` (`Session`).
- **`routing` cargo feature gate** — regex-based rule matching behind the default-on feature. Start: `Cargo.toml`. Files: `src/router/condition.rs`.
- **Opt-in features** — `cli-support`, `test-support`, and `import` (all off by default; see the Project Contract). Start: `Cargo.toml`. Files: `src/lib.rs`, `src/cli_support/mod.rs`, `src/test_support.rs`, `src/import/mod.rs`.
- **CLI support (`cli-support`)** — open-file limit raising (`fdlimit`), startup path preflight (`preflight`), `shutdown_signal`, the `app_version!` macro, and the shared `[mitm]` / `[logging]` config-file sections with their command-line merge (`file`: `FileMitmConfig`, `FileLoggingConfig`, `merge_mitm`, `merge_logging`, `FileValueError`). Start: `src/cli_support/mod.rs`. Files: `src/cli_support/fdlimit.rs`, `src/cli_support/preflight.rs`, `src/cli_support/file.rs`.
- **Test fixtures (`test-support`)** — test CA (`test_ca`, `TestCa`, `FixedCert`), echo server, HTTP/TLS origin, TLS client, `connect_through`, logger and access-log capture for consumers' tests. Start: `src/test_support.rs`.
- **`.bak` importer (`import`)** — converts AirSwitch / ZeroOmega / SwitchyOmega backups (`convert`) into profiles and rules, merges nested SwitchProfile conditions, and renders commented `rusty-conduit-switch` TOML (`render_toml`). Start: `src/import/mod.rs`. Files: `src/import/airswitch.rs`.
- **Pure-Rust crypto policy check** — fails if `openssl-sys` or `aws-lc-sys` is in the dependency graph. Start: `scripts/check-pure-rust-crypto.py`. Files: `Cargo.toml`, `Cargo.lock`.
- **Continuous integration** — format, clippy, no-default-features, crypto policy, and the test matrix on Linux, macOS, and Windows. Start: `.github/workflows/ci.yml`.
- **Release** — on a `v*` tag, re-runs verification and creates the GitHub Release from the annotated tag's message body. Start: `.github/workflows/release.yml`.
- **Integration tests** — end-to-end tests of counters, the custom resolver, MITM (including HTTP/2 upstream reconnects), and proxy error pages. Start: `tests/traffic_counters.rs`. Files: `tests/route_resolver.rs`, `tests/mitm_counters.rs`, `tests/mitm_h2_upstream_reconnect.rs`, `tests/error_pages.rs`.
- **Routing benchmark** — times `Router::route` over 100 / 1000 / 5000 generated rules (host, wildcard, mixed, URL); run with `cargo run --release --example route_bench`. Start: `examples/route_bench.rs`.
- **Divergence ledger and open items** — recorded differences from each consumer's former copy and engine changes every consumer inherits, plus open decisions. Start: `docs/DIVERGENCE-LEDGER.md`. Files: `docs/TODO.md`.

## Child DOX Index

- src/http/ — HTTP/HTTPS proxy engine: forward proxy, `CONNECT` tunnel, MITM interception.
- src/mitm/ — HTTPS MITM support: CA loading, leaf signing, leaf-certificate cache.
- src/router/ — Auto Switch rule matching, rule conditions, and routing validation.
- src/socks/ — SOCKS4/4a/5 server engine and the SOCKS web guard.
- src/upstream/ — upstream dialers, proxy chaining, failover, and profile registry.
