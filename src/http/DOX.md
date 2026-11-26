# src/http

## Purpose
HTTP/HTTPS proxy engine: serves each accepted HTTP connection with `hyper` 1.x and dispatches requests to the forward proxy, the opaque `CONNECT` tunnel, or MITM interception.

## Ownership
Owns code under `src/http/`. CA loading, leaf signing, and the leaf-certificate cache belong to ../mitm. Routing decisions come from ../router and ../routing.rs. Dialing belongs to ../upstream.

## Local Contracts
- `CONNECT host:port` is routed with `scheme = "https"` and no URL. Any other method is routed with `scheme = "http"` and the full URL.
- Forwarding to an `http`/`https` upstream proxy sends the request in absolute-form to the proxy itself, with `Proxy-Authorization` when configured — never `CONNECT host:80`. `direct`, `socks5`, and `socks4a` upstreams get origin-form over a dialed tunnel.
- Hop-by-hop headers are stripped before forwarding. Bodies are streamed, never cached.
- A `block` route or filter denial answers `403 Forbidden`. Dial errors map to HTTP status codes through `dial_error_to_status`.
- MITM dials upstream through the routing snapshot taken at `CONNECT` time and verifies upstream TLS with the platform verifier unless custom roots are given.
- Every request or tunnel emits one access record with `rule`, `profile`, and the redacted `upstream`, and updates the traffic counters.

## Feature Map
- **Request dispatch** — `HttpProxy` routes each request and dispatches by method; shared response, error, access-guard, and counting-body helpers. Start: `mod.rs`.
- **Forward proxy** — absolute-form HTTP forwarding with hop-by-hop stripping and failover-chain dialing. Start: `forward.rs`.
- **`CONNECT` tunnel (MITM off)** — validates `host:port`, dials the failover chain, replies `200 Connection Established`, then relays opaque bytes. Start: `tunnel.rs`. Files: `../net.rs`.
- **MITM interception (MITM on)** — reads the `ClientHello` (SNI/ALPN), serves a signed leaf, opens upstream TLS, and routes each decrypted request by URL. Start: `mitm_tunnel.rs`. Files: `../mitm/`.

## Child DOX Index
- (none)
