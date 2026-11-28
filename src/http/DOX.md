# src/http

## Purpose
HTTP/HTTPS proxy engine: serves each accepted HTTP connection with `hyper` 1.x and dispatches requests to the forward proxy, the opaque `CONNECT` tunnel, or MITM interception.

## Ownership
Owns code under `src/http/`. CA loading, leaf signing, and the leaf-certificate cache belong to ../mitm. Routing decisions come from ../router and ../routing.rs. Dialing belongs to ../upstream.

## Local Contracts
- `CONNECT host:port` is routed with `scheme = "https"` and no URL. Any other method is routed with `scheme = "http"` and the full URL.
- When MITM is enabled and a `CONNECT` matches either `MitmOptions::bypass_hosts` (`MitmInterceptor::should_bypass`) or a rule with `RuleConfig::bypass_mitm` (`RouteDecision::bypass_mitm == true`), dispatch hands the resolved target to the opaque `CONNECT` tunnel (`tunnel::handle_resolved`, `mitm=false`) instead of intercepting TLS.
- Forwarding to an `http`/`https` upstream proxy sends the request in absolute-form to the proxy itself, with `Proxy-Authorization` when configured — never `CONNECT host:80`. `direct`, `socks5`, and `socks4a` upstreams get origin-form over a dialed tunnel.
- Hop-by-hop headers are stripped before forwarding, except `te: trailers` on decrypted HTTP/2 (the one `TE` value HTTP/2 allows). Bodies are streamed, never cached.
- A `block` route or filter denial answers `403 Forbidden`. Dial errors map to HTTP status codes through `dial_error_to_status`.
- Every proxy-generated error response is rendered from the configured `ErrorPageTemplate` (`HttpProxy::error_page`, or the interceptor's override for decrypted MITM requests) as `text/html; charset=utf-8` with `Cache-Control: no-store`. Placeholder values are always HTML-escaped, and the target never carries userinfo. Responses from an upstream origin, error statuses included, are never rewritten, wrapped, or inspected for this.
- MITM dials upstream through the routing snapshot taken at `CONNECT` time and verifies upstream TLS per `MitmOptions::upstream_tls` (`upstream_client_config`): `Strict` (platform verifier, the default), `AllowInsecureHosts` (skip certificate verification only for targets matching a `FilterRule`; an empty list is strict), or `InsecureAll`. An embedder may instead inject its own `ClientConfig` (`MitmState::build`, `from_pem_bytes_with_upstream`).
- Insecure upstream modes skip only certificate validation: TLS 1.2/1.3 handshake signatures are always verified with the provider's algorithms.
- A MITM tunnel holds at most one upstream connection at a time and redials it through the tunnel's decision when the origin closes it, on HTTP/1.1 and HTTP/2 alike: the client connection outlives any one upstream connection. On HTTP/2, a request dropped unanswered is resent once only if it was never written, or if it is idempotent and has no body. If a redial no longer negotiates `h2`, the client is sent GOAWAY and requests in hand go out over HTTP/1.1. Decrypted HTTP/2 accepts header blocks up to `H2_MAX_HEADER_LIST_SIZE` (256 KiB) from both the client and the origin, never hyper's 16 KiB default.
- Every request or tunnel emits one access record with `rule`, `profile`, and the redacted `upstream`, and updates the traffic counters.

## Feature Map
- **Request dispatch** — `HttpProxy` routes each request and dispatches by method; shared response, error, access-guard, and counting-body helpers. Start: `mod.rs`.
- **Proxy error pages** — `ErrorPageTemplate`: the built-in unbranded HTML page or a custom template with `{{status}}`, `{{reason}}`, `{{message}}`, `{{target}}`, used for every proxy-generated error (`HttpProxy::with_error_page`, `MitmInterceptor::with_error_page`, `EngineHandlers::with_error_page`). Start: `error_page.rs`. Files: `mod.rs`, `forward.rs`, `tunnel.rs`, `mitm_tunnel.rs`, `../server.rs`, `../../tests/error_pages.rs`.
- **Forward proxy** — absolute-form HTTP forwarding with hop-by-hop stripping and failover-chain dialing. Start: `forward.rs`.
- **`CONNECT` tunnel (MITM off)** — validates `host:port`, dials the failover chain, replies `200 Connection Established`, then relays opaque bytes. Start: `tunnel.rs`. Files: `../net.rs`.
- **MITM interception (MITM on)** — reads the `ClientHello` (SNI/ALPN), serves a signed leaf, opens upstream TLS, and routes each decrypted request by URL; HTTP/2 tunnels redial the origin when it closes its connection (`H2Tunnel`). Start: `mitm_tunnel.rs`. Files: `../mitm/`, `../../tests/mitm_h2_upstream_reconnect.rs`.
- **MITM upstream TLS verification** — `upstream_client_config` builds the upstream `ClientConfig` for each `UpstreamTlsVerification` mode (`Strict`, `AllowInsecureHosts` via `SelectiveServerCertVerifier`, `InsecureAll`). Start: `mitm_tunnel.rs`. Files: `../mitm/mod.rs`, `../filter.rs`.

## Child DOX Index
- (none)
