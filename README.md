# rusty-conduit-engine

The shared proxy and routing engine behind the rusty-conduit CLI family.

## Purpose

`rusty-conduit-engine` is a library-only Rust crate (`rusty_conduit_engine`) that holds the
HTTP/HTTPS and SOCKS proxy engine, the routing types, and the optional HTTPS MITM support shared by
the rusty-conduit command-line tools. Keeping one engine means a fix or feature lands once and every
tool gets it.

## Consumers

- [`rusty-conduit`](../rusty-conduit)
- [`rusty-conduit-ssh`](../rusty-conduit-ssh)
- [`rusty-conduit-switch`](../rusty-conduit-switch)

CLI-only concerns (argument parsing, config file formats, file watching) stay in those tools; the
engine does not depend on `clap`, `toml`, or `notify`.

## Features

- `routing` (default): regex-based rule matching (pulls in `regex`). It covers `host_wildcard`,
  `host_regex`, `url_wildcard`, `url_regex`, and wildcard `bypass` hosts.

With `--no-default-features`, every module still builds, including `RoutingState` and the
`RouteResolver` trait. Rules that use a regex-based condition then fail to compile, with an error
that names the `routing` feature. Exact-host, CIDR/IP, port, host-level, weekday, time, keyword,
inbound, and `<local>`/IP/CIDR bypass conditions keep working.

## Modules

`routing` (profile and rule model, `RoutingState` snapshots, counters, `RouteResolver`), `router`
(rule matching and validation), `upstream` (direct, HTTP, SOCKS4a/5, block, and external dialers),
`http`, `socks`, `mitm` (HTTPS MITM, CA loading from files or PEM bytes), `server` and `mux`
(listeners), `filter`, `net`, and `logging` (log lines and the `AccessRecord` sink).

## Pure-Rust crypto policy

TLS and certificate handling use `rustls` with the `ring` provider, plus `rcgen`, `x509-parser`,
and `p12-keystore`. OpenSSL (`openssl-sys`) and `aws-lc` (`aws-lc-sys`) must never enter the
dependency graph on any target platform. CI enforces this with:

```sh
python3 scripts/check-pure-rust-crypto.py
```

which prints `PURE_RUST_CRYPTO_OK` on success.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo check --no-default-features
cargo test
cargo test --no-default-features
python3 scripts/check-pure-rust-crypto.py
```

## Plan

See [`docs/MASTER-PLAN.md`](docs/MASTER-PLAN.md) for the roadmap and milestones, and
[`docs/DIVERGENCE-LEDGER.md`](docs/DIVERGENCE-LEDGER.md) for the recorded differences from each
consumer's former copy, with their decisions.

## License

BSD-3-Clause. See [`LICENSE`](LICENSE).
