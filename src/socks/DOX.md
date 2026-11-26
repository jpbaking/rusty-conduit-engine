# src/socks

## Purpose
SOCKS4/4a/5 server engine wired to the Auto Switch router, plus the relay-guard hook used for SOCKS web blocking.

## Ownership
Owns code under `src/socks/`. Routing decisions come from ../router and ../routing.rs. Dialing belongs to ../upstream.

## Local Contracts
- Only TCP `CONNECT` is supported (IPv4, IPv6, domain). There is no inbound authentication. The SOCKS4 user ID is metadata only.
- Every request is routed with `inbound = socks`, no scheme, and no URL. Hostnames routed to a proxy upstream are forwarded unresolved.
- A `block` route or filter denial answers SOCKS5 `0x02` or SOCKS4 `0x5B`. Other reply codes follow `v4.rs` and `v5.rs`.
- A `RelayGuard` inspects only the first client-to-upstream chunk. Upstream-to-client bytes are never delayed, so server-speaks-first protocols keep working.
- Every session emits one `SocksLogRecord` with `rule`, `profile`, and the redacted `upstream`, and updates the traffic counters.

## Feature Map
- **SOCKS session engine** — `SocksProxy`: handshake dispatch, routing, failover dialing, relay, guard hook, and logging. Start: `mod.rs`.
- **SOCKS4/4a protocol** — request parsing and replies. Start: `v4.rs`.
- **SOCKS5 protocol** — RFC 1928 No-Auth greeting, request parsing, and reply codes. Start: `v5.rs`.
- **SOCKS web blocking** — `WebGuard` blocks plaintext HTTP and HTTPS `ClientHello`s over SOCKS (`--socks-block-web`). Start: `web_guard.rs`.

## Child DOX Index
- (none)
