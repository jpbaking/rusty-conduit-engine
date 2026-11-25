# Rusty Conduit Engine — Master Plan

> **Status:** Draft v1 (Extraction & Migration Plan)
> **Crate:** `rusty-conduit-engine` (library only, `rusty_conduit_engine`, BSD-3-Clause)
> **Consumers:** [`rusty-conduit`](../rusty-conduit) (v1.2.0), [`rusty-conduit-ssh`](../rusty-conduit-ssh) (v1.3.0), [`rusty-conduit-switch`](../rusty-conduit-switch) (v0.0.1-beta.1), and [`rusty-conduit-gui`](../rusty-conduit-ssh-gui) (formerly `rusty-conduit-ssh-gui`; see its master plan)
> **Work branch:** `feat/one-engine` in each of the three CLI repositories. This repository is new and is developed on `main`.

---

## 1. Why

The three CLIs are forks of one proxy front end. Every bug fix and every security review of the shared code is currently done three times, and the copies keep drifting. Divergence of each file from `rusty-conduit` (lines changed / base size), measured 2026-11-25:

| Module | `-ssh` | `-switch` | Notes |
| :--- | ---: | ---: | :--- |
| `mitm/ca.rs` | 0 / 800 | 0 / 800 | Identical. The CA loader bugs in `rusty-conduit-ssh-gui/docs/TODO.md` exist in all three. |
| `filter.rs`, `mux.rs` | 0 | 0 | Identical. |
| `mitm/cache.rs` | 2 / 504 | 8 / 504 | |
| `logging.rs` | 52 / 1023 | 50 / 1023 | |
| `server.rs` | 52 / 826 | 159 / 826 | |
| `socks/*`, `http/forward.rs`, `http/tunnel.rs` | 14–45 | 117–396 | `-switch` passes a route request to a router per connection. |
| `http/mitm_tunnel.rs` | 84 / 1107 | 961 / 1107 | `-switch` adds URL routing and upstream TLS over routed upstreams. |
| `net.rs` | 755 / 360 | 115 / 360 | `-ssh` replaces the dialer with tunnel-state-aware SOCKS5-to-`ssh -D`. |
| `config.rs` | 1556 / 1535 | 2133 / 1535 | Product-specific by nature; stays in each CLI. |

The pattern: the protocol front end is shared; products differ only in **which dialer serves a connection** (`-ssh`: always the tunnel; `-switch`: the router decides; `rusty-conduit`: always direct) and in their **CLI / TOML layer**.

`-switch` already has the general design (route request → resolved target → dialer, with a failover chain), so the engine is extracted **from `rusty-conduit-switch`**, and the other two are ported onto it.

## 2. Goals & Non-Goals

### Goals
1. One crate owns listeners, mux, HTTP forward / `CONNECT`, SOCKS4/4a/5, HTTPS MITM, the allow/block filter, TCP keepalive, logging, graceful shutdown, routing, upstream dialers, and atomic hot-reload.
2. **No user-visible behavior change** in any CLI, except fixes that are listed in each CLI's changelog (see §6.2). Every CLI's existing test suite passes unmodified, apart from import paths.
3. The extension points the GUI needs (§4) exist, so `rusty-conduit-gui` embeds only the engine plus its own SSH code.

### Non-Goals
- No new end-user features in the CLIs during the migration (feature work is frozen on the three `main` branches, or cherry-picked onto `feat/one-engine` — see §7).
- No monorepo. Each CLI keeps its repository, releases, Docker image, and CLI/TOML schema.
- No CLI argument parsing, config-file loading, file watching, signal handling, or SSH process supervision in the engine.

## 3. Crate Boundaries

```text
rusty-conduit-engine (this repo)
├── net          KeepaliveSettings, PrefixedStream, relay_bidirectional
├── filter       HostFilter (allow/block: IP, CIDR, FQDN, *.domain)
├── mux          single-port HTTP/SOCKS sniffing
├── http         forward, CONNECT tunnel, MITM tunnel
├── socks        v4/4a, v5, web_guard (socks_block_web_in_mitm)
├── mitm         CA loading (PEM, PKCS#12, + from bytes), leaf-cert cache, reaper
├── logging      Logger (6 levels), AccessLogger (text/json), + structured record sink
├── upstream     UpstreamDialer, UpstreamStream, Direct/Block/HttpConnect/Socks5/Socks4a,
│                FailoverDialer, ProfileRegistry, + external dialers
├── router       Router, RouteRequest, ParsedTarget, conditions, rule validation
├── routing      RoutingConfig / ProfileConfig (serde types), RoutingState,
│                SharedRouting (ArcSwap), RouteResolver
└── server       Server, EngineHandlers, Connection, ShutdownReport

rusty-conduit         CLI + TOML → engine; routing = fixed `direct` default, no rules
rusty-conduit-ssh     CLI + TOML → engine; ssh supervisor, TunnelState, Socks5-to-`ssh -D`
                      dialer (registered as an external dialer), ReverseProxy (a second
                      engine Server with direct egress, exposed via `ssh -R`)
rusty-conduit-switch  CLI + TOML → engine; reload triggers (SIGHUP, --watch-config),
                      `validate`, `test-route`, `import-airswitch`
```

Routing lives in the engine (not in `-switch`) because the GUI needs it, and because it makes two items of `rusty-conduit/FUTURE-PLANS.md` (§5 upstream proxy chaining, §6 config hot-reload) cheap to add to `rusty-conduit` after the migration. The routing types are gated behind a default-on `routing` cargo feature so a consumer can opt out if binary size ever matters.

## 4. Extension Points (new API)

1. **Boxed upstream streams.** Add `UpstreamStream::Boxed(Box<dyn AsyncRead + AsyncWrite + Send + Unpin>)`. An SSH `direct-tcpip` channel plugs in directly, removing the per-connection loopback TCP pair the GUI's `RusshDialer` uses today.
2. **External dialers.** `ProfileRegistry::with_external(name, Arc<dyn UpstreamDialer>)` registers a dialer under a profile name that the config declares as `type = "external"` (no host/port). Validation accepts a rule or chain referencing it only if it was declared; building `RoutingState` fails with a clear error if a declared external dialer was not supplied. `-ssh` uses this for its tunnel; the GUI uses it for each SSH tunnel.
3. **`RouteResolver` trait.** `fn resolve(&self, req: &RouteRequest) -> Result<RouteDecision, AccessDenied>`, implemented by `RoutingState`. Handlers depend on the trait, so a consumer can supply its own resolver (the GUI's temporary tray overrides use this, or swap a rule-less `RoutingState`).
4. **MITM CA from bytes.** `MitmState::from_pem_bytes(cert_chain_pem, key_pem, opts)` so the GUI no longer stages a vaulted CA in a temporary file.
5. **Observability hooks.** An `AccessRecord` sink (channel or callback) carrying structured records alongside the formatted log line; per-profile and per-rule atomic counters (`active`, `bytes_in`, `bytes_out`, `hits`) readable from `RoutingState`.
6. **Typed dial errors.** Keep `io::Error` at the boundary, but carry a typed inner error (blocked, filtered, not-connected, auth-rejected, timeout) so all consumers map to the same HTTP `502`/`504` and SOCKS reply codes.

## 5. Dependency & Release Mechanics

- **During development:** consumers use `rusty-conduit-engine = { path = "../rusty-conduit-engine" }` on `feat/one-engine` (same sibling-folder convention as today's GUI).
- **For releases:** consumers switch to a pinned git dependency, `{ git = "https://github.com/jpbaking/rusty-conduit-engine", tag = "vX.Y.Z" }`. Local development then overrides it with a `[patch."https://github.com/jpbaking/rusty-conduit-engine"]` entry in a gitignored `.cargo/config.toml`.
- **Why not path dependencies at release:** each CLI's Dockerfile builder stage copies only its own repository (`COPY Cargo.toml Cargo.lock ./`, `COPY src ./src`), and each CI checks out one repository. A git tag dependency keeps both working unchanged.
- **If this repository is private:** CI and Docker builds need a read token for the git dependency. Decide before E5.
- **Versioning:** semver, starting at `0.1.0`. Consumers pin exact tags. The engine has no compatibility promise to outside users before `1.0`.

## 6. Milestones

### E0 — Bootstrap
- Create the repository: `Cargo.toml` (lib only), `LICENSE` (BSD-3-Clause), `README.md`, this plan.
- CI on Linux, macOS, Windows: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and the pure-Rust crypto check (`ring` only; no OpenSSL / `aws-lc`), copied from `rusty-conduit-ssh-gui/scripts/check-pure-rust-crypto.py`.
- Create `feat/one-engine` from `main` in `rusty-conduit`, `rusty-conduit-ssh`, and `rusty-conduit-switch`.

### E1 — Extract from `rusty-conduit-switch`
- Move the modules in §3 out of `rusty-conduit-switch` (with their unit tests) into the engine, unchanged.
- On `rusty-conduit-switch/feat/one-engine`, depend on the engine and keep the old module paths as re-exports (`pub use rusty_conduit_engine::router;`) so integration tests and downstream code compile unchanged.
- **Done when:** the engine's tests and `rusty-conduit-switch`'s full suite pass, `cmd_test_route` output is byte-identical on the repository's fixtures, and clippy is clean in both.

### E2 — Extension points & shared fixes
- Implement §4.1–§4.6, with tests for each.
- Fix the two MITM CA loader issues from `rusty-conduit-ssh-gui/docs/TODO.md` (EC keys with explicit curve parameters; PBES2 with the HMAC-SHA1 PRF, or a corrected hint). They are fixed once here, for all consumers.
- Tag `v0.1.0`. **This tag is the gate for the GUI's engine migration** (GUI phase U1).

### E3 — Port `rusty-conduit`
- Replace its proxy modules with the engine; map its `Config` to a `RoutingState` with `default = direct` and no rules, plus its filter, MITM, and keepalive settings.
- **Done when:** its existing test suite passes; `--help`, config precedence (`CLI > env > TOML`), log formats, and exit codes are unchanged; idle RSS stays within 10% of v1.2.0 (~6 MB).

### E4 — Port `rusty-conduit-ssh`
- Keep `ssh.rs` (supervisor, backoff), `TunnelState`, `Socks5UpstreamDialer`, and its config in the crate. Register the tunnel dialer as an external profile and route everything to it.
- Rebuild `ReverseProxy` as a second engine `Server` with a direct-egress `RoutingState` and the reverse allow/block filter.
- **Done when:** its existing test suite passes, including reconnect and reverse-proxy tests; CLI, config, logs, and exit codes are unchanged.

### E5 — Release
- Reconcile the per-module divergence ledger (§6.2) and publish it in each CLI's changelog.
- Switch consumers from path to git-tag dependencies; confirm Docker builder and prebuilt stages and release workflows pass.
- Merge `feat/one-engine` into `main` in all three repositories, in this order: `-switch`, `rusty-conduit`, `-ssh`. Release each (minor version bumps, since behavior is unchanged).

### 6.2 Divergence ledger (part of E1–E4)
Where a "shared" module differs between the three copies (for example the ~50 differing lines in `logging.rs`, or the `socks/*` differences in `-ssh`), each difference is recorded with a decision: **keep** (becomes engine behavior), **option** (becomes an engine setting the consumer sets to its old behavior), or **fix** (a bug in one copy; the change is listed in that CLI's changelog). No difference is dropped silently.

## 7. Risks

| Risk | Mitigation |
| :--- | :--- |
| `main` keeps moving in a CLI while `feat/one-engine` is open | Freeze features on the three `main` branches until E5; cherry-pick urgent fixes onto `feat/one-engine` and into the engine. |
| Silent behavior change in a CLI | §6.2 ledger; unchanged test suites; byte-identical `test-route` output. |
| Dynamic dispatch cost of `RouteResolver` / boxed streams | `-switch` already dispatches per connection through `Arc<dyn UpstreamDialer>`; boxing applies only to external dialers. Spot-check throughput against the current releases. |
| Binary size of plain `rusty-conduit` grows (router, regex) | Measure in E3; if it matters, build `rusty-conduit` with `default-features = false` and a direct-only resolver. |
| Private-repository git dependency in CI/Docker | Decide visibility before E5 (§5). |

## 8. Open Decisions

1. **Repository visibility** for `rusty-conduit-engine` (public makes git dependencies and Docker builds trivial).
2. **Publishing to crates.io** later (would replace git-tag dependencies), or keep git tags.
3. **`external` profile type in TOML** — expose it in `-switch`'s config schema (useful only to embedders), or keep it API-only and invisible to CLI users. *(Recommendation: API-only.)*
