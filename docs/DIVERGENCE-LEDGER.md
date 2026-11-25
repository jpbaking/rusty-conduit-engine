# Divergence Ledger

This ledger implements [`MASTER-PLAN.md` §6.2](MASTER-PLAN.md). Where a shared module differs
between the engine and a consumer's former copy (`rusty-conduit`, `rusty-conduit-ssh`,
`rusty-conduit-switch`), or where the engine changes behavior a consumer relied on, the difference is
recorded here with a decision. No difference is dropped silently. In E5 each consumer's entries are
copied into that CLI's changelog.

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

## E3–E4 (to be filled)

Entries for `rusty-conduit` (E3) and `rusty-conduit-ssh` (E4) are added here as each port finds them.
