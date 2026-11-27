# Divergence Ledger

Where a shared module differs between the engine and a consumer's former copy
(`rusty-conduit`, `rusty-conduit-ssh`, `rusty-conduit-switch`), or where the engine changes
behavior a consumer relied on, the difference is recorded here with a decision. No difference is
dropped silently. Each consumer's entries are listed in that CLI's release notes.

## Decisions

| Decision | Meaning |
| :--- | :--- |
| **keep** | The behavior becomes engine behavior for every consumer. |
| **option** | The behavior becomes an engine setting; a consumer sets it to keep its old behavior. |
| **fix** | A bug in one copy. The engine carries the corrected behavior, and the change is listed in that CLI's changelog. |

## Entry format

Each entry records: an ID, the module, the consumers affected, what differs, the decision, and where
it landed (milestone and test).

## E1 — Extraction baseline (`rusty-conduit-switch`)

The engine was extracted from `rusty-conduit-switch` `main` at `69b0c41` (`chore: add CI/Release
workflows and set version to 0.0.1-beta.1`). On `feat/one-engine` (`1e317d4`), `-switch` depends on
the engine and re-exports the old module paths.

| ID | Module | Consumers | Difference | Decision | Landed |
| :--- | :--- | :--- | :--- | :--- | :--- |
| E1-01 | `filter`, `http`, `logging`, `mitm`, `mux`, `net`, `router`, `server`, `socks`, `upstream` | `-switch` | None. Moved unchanged, with their unit tests. | keep | E1. `-switch` suite passes, and `test-route` output is byte-identical to `main` on its fixtures. |
| E1-02 | `routing` (config and snapshot types) | `-switch` | Config-file loading stays in the CLI. The engine takes a `RoutingInput` and returns a typed `RoutingError`; `-switch` maps it with `From<RoutingError>`. Error text is unchanged. | keep | E1 |
| E1-03 | `mitm`, `server` | `-switch` | The CLI's MITM and listener settings are passed as engine structs (`MitmOptions`, `ServerOptions`) instead of the CLI `Config`. Behavior is unchanged. | keep | E1 |

The `rusty-conduit` and `rusty-conduit-ssh` copies are compared module by module in E3 and E4.
Known areas to reconcile then: about 50 differing lines in `logging.rs`, and the `socks/*`
differences in `-ssh`. Each will get its own entry below.

## E2 — Extension points and shared fixes

| ID | Module | Consumers | Difference | Decision | Landed |
| :--- | :--- | :--- | :--- | :--- | :--- |
| E2-01 | `mitm::ca` | all, GUI | Unencrypted EC private keys with **explicit curve parameters** were rejected. Explicit parameters equal to P-256 or P-384 are now rewritten to the named curve and load. Any other explicit curve fails with `CaError::ExplicitEcParameters`, whose hint gives the `openssl ec -param_enc named_curve` fix. | fix | E2 (`src/mitm/ca.rs`) |
| E2-02 | `mitm::ca` | all, GUI | Keys encrypted with **PBES2 using the HMAC-SHA1 PRF** (for example LibreSSL `openssl pkcs8 -topk8 -v2 aes-256-cbc`) failed with a misleading hint. They now fail early with `CaError::Pbes2Sha1Prf`, whose corrected hint names the OpenSSL 3 `-v2prf hmacWithSHA256` or `-v1 PBE-SHA1-3DES` re-encryption, or PKCS#12. No new crypto dependency was added. | fix | E2 (`src/mitm/ca.rs`) |
| E2-03 | `upstream` | all | New `UpstreamStream::Boxed` variant and API-only `ProfileType::External` dialers, registered with `with_external`. They are additive, and config files cannot declare them. | keep | E2 |
| E2-04 | `router`, `http`, `socks`, `server` | all | Handlers depend on the `RouteResolver` trait instead of `RoutingState` directly. `RoutingState` implements it, so behavior is unchanged. | keep | E2 |
| E2-05 | `upstream`, `http`, `socks` | all | Dial failures carry a typed `DialError` inside `io::Error`. HTTP `502`/`504` and SOCKS reply codes are unchanged. | keep | E2 |
| E2-06 | `logging`, `routing` | all | Additive `AccessRecord` sink and per-profile and per-rule `RoutingCounters` (`active`, `bytes_in`, `bytes_out`, `hits`). Formatted log lines are unchanged. | keep | E2 |
| E2-07 | `mitm` | GUI | `MitmState::from_pem_bytes` loads a CA from memory, so a vaulted CA no longer has to be staged in a temporary file. File loading is unchanged. | keep | E2 |
| E2-08 | `router::condition` | all | The default-on `routing` cargo feature gates the `regex` dependency. With `--no-default-features`, the regex-based conditions (`host_wildcard`, `host_regex`, `url_wildcard`, `url_regex`, and wildcard `bypass` hosts) fail to compile and return a `ConditionError` that names the feature. All other conditions and modules keep working. Default builds are unchanged. | option | E2 (`src/router/condition.rs`) |

## E3 — Port `rusty-conduit`

`rusty-conduit` v1.2.0 (`db2c82a`) was ported onto `rusty-conduit-engine` (`default-features = false`) on `feat/one-engine` (`f5fbce5`). All 71 tests pass with zero assertion changes and byte-identical `--help` output.

| ID | Module | Consumers | Difference | Decision | Landed |
| :--- | :--- | :--- | :--- | :--- | :--- |
| E3-01 | `logging` | `rusty-conduit` | The engine emits `rule`, `profile`, and `upstream` fields by default; `rusty-conduit` v1.2.0 did not. Added `AccessLogConfig::with_routing_fields(false)` / `AccessLogger::with_routing_fields(false)` so `rusty-conduit` keeps its exact v1.2.0 text and JSON schemas. | option | E3 (`src/logging.rs`, `bb884a2`) |
| E3-02 | `filter`, `upstream::direct` | `rusty-conduit` | `Router::route_with` runs `HostFilter::check_host` without DNS (required for remote-DNS proxied upstreams). `rusty-conduit` passes `RoutingInput.filter = None` and installs its `HostFilter` on `DialerOptions` (`DirectDialer`) so IP/CIDR-only allowlists continue checking resolved IPs at dial time. | option | E3 (`f5fbce5`) |
| E3-03 | `http`, `socks` | `rusty-conduit` | Bad `CONNECT` target `400` body changed from `invalid CONNECT target` to `CONNECT target must be host:port`; diagnostic `ERROR`/`INFO`/`DEBUG` wording now uses the engine's unified format. HTTP status codes (`403`/`502`/`504`) and SOCKS reply codes are unchanged. | keep | E3 (`f5fbce5`) |
| E3-04 | `server`, `mitm` | `rusty-conduit` | Engine defaults (`max_connections = 1024`, `mitm_cert_cache_max = 4096`) differ from `rusty-conduit`'s (`4096`, `10_000`); `rusty-conduit` passes its own values explicitly via `ServerOptions` / `MitmOptions`. Inherits E2 CA-loader fixes (`E2-01`, `E2-02`). | option / fix | E3 (`f5fbce5`) |

## E4 — Port `rusty-conduit-ssh`

`rusty-conduit-ssh` v1.3.0 (`88bbe0c`) was ported onto `rusty-conduit-engine` (`default-features = false`) on `feat/one-engine` (`7b6daa4`). All 141 tests pass (including reconnect and reverse-proxy tests) with zero assertion changes and byte-identical `--help` and error exit codes.

| ID | Module | Consumers | Difference | Decision | Landed |
| :--- | :--- | :--- | :--- | :--- | :--- |
| E4-01 | `logging` | `-ssh` | `-ssh` tags every access record with `direction=forward\|reverse` right after `kind=` and omits `rule`/`profile`/`upstream`. Added `ProxyDirection` and `AccessLogger::with_direction(...)` alongside `with_routing_fields(false)` so forward and reverse servers share one writer while keeping the exact v1.3.0 log schema. | option | E4 (`src/logging.rs`, `bb884a2`) |
| E4-02 | `net`, `upstream` | `-ssh` | `-ssh` keeps `TunnelState` and `Socks5UpstreamDialer` in a thin `src/net.rs`, implementing `rusty_conduit_engine::upstream::UpstreamDialer` (`UpstreamStream::Tcp`, typed `DialError::NotConnected`/`Timeout`) and registering it as an API-only `ProfileType::External` profile (`ssh-tunnel`). Forward `HostFilter` runs in the router without local DNS, matching former `FilteredDialer`. | keep | E4 (`7b6daa4`) |
| E4-03 | `reverse`, `server`, `filter` | `-ssh` | `ReverseProxy` is rebuilt as a second engine `Server` with a direct-egress `RoutingState` and its reverse `HostFilter` passed via `DialerOptions` only (so IP/CIDR-only allowlists check resolved IPs at dial time, verified by `reverse_filter_checks_resolved_ips_for_hostnames`). | keep | E4 (`7b6daa4`) |
| E4-04 | `http`, `socks`, `mitm` | `-ssh` | Status/reply mapping now flows through typed `DialError` with identical HTTP (`403`/`502`/`504`) and SOCKS (`0x01`–`0x05`) codes; `MitmState` dials through routing instead of taking a dialer directly. Inherits E2 CA-loader fixes (`E2-01`, `E2-02`). | keep / fix | E4 (`7b6daa4`) |

## M — Maintenance after the CLI ports

Engine changes made after E5 that change behaviour every consumer inherits.

| ID | Module | Consumers | Difference | Decision | Landed |
| :--- | :--- | :--- | :--- | :--- | :--- |
| M-01 | `server` | all | Listeners are now bound through `bind_listener`, which requests a listen backlog of `LISTEN_BACKLOG` (4096) instead of the 128 that `TcpListener::bind` requests. The kernel still caps it (`net.core.somaxconn`, 4096 by default on Linux ≥ 5.4; `kern.ipc.somaxconn`, 128 by default on macOS). `SO_REUSEADDR` stays on for non-Windows platforms. Connection bursts larger than 128 no longer overflow the queue on Linux. | keep | `src/server.rs` (`bind_listener`, `backlog_queues_more_than_128_connections`) |
| M-02 | `logging` | all, GUI | Access-log files (`--http-access-log`, `--socks-log`) were flushed only when the 8 KiB write buffer filled or at shutdown, so a quiet proxy's file could stay empty and a hard kill lost up to 8 KiB. The writer task now drains every queued record and then flushes once the queue is empty: bursts still write in large blocks, and idle records reach the file at once. `stdout` output was already flushed per record. | fix | `src/logging.rs` (writer task, `file_sinks_flush_when_the_queue_drains`) |
| M-03 | `mitm`, `http` | all | `MitmOptions` gains `upstream_tls: UpstreamTlsVerification`, and the MITM upstream TLS connection can now skip certificate verification. `Strict` (the default) keeps the former behaviour: the OS trust store verifies every upstream certificate. `AllowInsecureHosts(Vec<FilterRule>)` accepts invalid or self-signed certificates only for targets that match an exact domain, `*.domain`, IP, or CIDR rule and verifies every other target strictly; an empty list is strict. `InsecureAll` accepts any upstream certificate ("blind trust", unsafe). Both insecure modes still verify TLS 1.2/1.3 handshake signatures. `MitmState::from_options` and `from_pem_bytes` follow the option; `build`, `from_pem_bytes_with_upstream`, `platform_client_config`, and `client_config_with_roots` are unchanged. Adding the field breaks code that builds `MitmOptions` with a struct literal and no `..Default::default()` (released as 0.5.0). | option | `src/mitm/mod.rs` (`UpstreamTlsVerification`), `src/http/mitm_tunnel.rs` (`upstream_client_config`, `SelectiveServerCertVerifier`, handshake tests) |
| M-04 | `http`, `server` | all, GUI | Errors the proxy generates itself (`400`, `403`, `502`, `504` on the forward, `CONNECT`, and decrypted MITM paths) were short `text/plain` bodies. They are now HTML pages (`Content-Type: text/html; charset=utf-8`, `Cache-Control: no-store`) rendered from an `ErrorPageTemplate`: by default a small unbranded built-in page with no external resources or scripts. Embedders can supply their own template (`from_template`, `from_file`) with `{{status}}`, `{{reason}}`, `{{message}}`, and `{{target}}` placeholders, each HTML-escaped, through `EngineHandlers::with_error_page`, `HttpProxy::with_error_page`, or `MitmInterceptor::with_error_page` (overrides the engine's template for decrypted requests). The target is `host` or `host:port` with any userinfo removed. `error_response` keeps its signature and renders the built-in page. Status codes are unchanged, and upstream responses, error statuses included, still pass through unchanged. | option | `src/http/error_page.rs` (`ErrorPageTemplate`), `src/http/mod.rs`, `src/http/forward.rs`, `src/http/tunnel.rs`, `src/http/mitm_tunnel.rs`, `src/server.rs` (`EngineHandlers::with_error_page`), `tests/error_pages.rs` |
