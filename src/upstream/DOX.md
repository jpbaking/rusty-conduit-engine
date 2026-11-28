# src/upstream

## Purpose
Upstream dialer and proxy-chaining layer: every outbound connection goes through an `UpstreamDialer` that returns an `UpstreamStream`.

## Ownership
Owns code under `src/upstream/`. Profile types and the `RoutingState` that builds dialers live in ../routing.rs. Plain TCP dialing helpers live in ../net.rs.

## Local Contracts
- Every outbound connection goes through an `UpstreamDialer`. Never dial around this layer from the protocol engines.
- `direct` resolves through the OS/Tokio resolver on every dial: no DNS cache, no pooling. Hostnames routed to a proxy are always sent unresolved.
- `block` returns a typed `RouteBlocked` error with no DNS or network I/O.
- `FailoverDialer` advances only on a TCP connect or proxy handshake failure, never on `RouteBlocked`.
- Passive failover cooldown: in a multi-entry chain, a profile whose dial failed is skipped for the registry's cooldown (`DEFAULT_FAILOVER_COOLDOWN`, 3 s; `ProfileRegistry::with_failover_cooldown`, `Duration::ZERO` disables) while another entry is not cooling; one dial probes it after expiry and success clears the mark. Skipped entries are still tried last, so the cooldown alone never fails a dial. `RouteBlocked` and filter denials never mark a profile. The marks are per-profile failure timestamps, not a connection or DNS cache.
- `with_timeout` takes its error context as a closure, evaluated only on timeout.
- `https` proxies use `tokio-rustls` (`ring` provider) verified with `rustls-platform-verifier` unless custom trust anchors or `tls_insecure_skip_verify` are set.
- Credentials never appear in `Debug` output, logs, or error strings.
- External dialers are API-only: the `external` profile type cannot be declared in a config file.

## Feature Map
- **Dialer trait, streams, errors, failover, registry** — `UpstreamDialer`, `UpstreamStream` (vectored writes), `DialError`, `dialer_for_profile`, `ProfileRegistry` (failover cooldown), `FailoverDialer`, external-dialer plumbing. Start: `mod.rs`.
- **`direct` dialer** — OS-resolver connect (Happy Eyeballs across multiple addresses) with `TCP_NODELAY` and keepalive. Start: `direct.rs`. Files: `../net.rs`.
- **`block` dialer** — refuses without touching DNS or the network. Start: `block.rs`.
- **`http`/`https` `CONNECT` dialer** — `CONNECT` tunnel client with optional Basic auth and TLS to the proxy. Start: `http.rs`.
- **`socks5` dialer** — RFC 1928 client with optional RFC 1929 auth and remote DNS. Start: `socks5.rs`.
- **`socks4a` dialer** — SOCKS4a client with remote DNS. Start: `socks4a.rs`.

## Child DOX Index
- (none)
