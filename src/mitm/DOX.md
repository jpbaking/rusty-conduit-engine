# src/mitm

## Purpose
HTTPS MITM support: loads the user-supplied CA, signs per-host leaf certificates, and caches them. The interception itself lives in ../http/mitm_tunnel.rs.

## Ownership
Owns code under `src/mitm/`. TLS interception of `CONNECT` traffic is owned by ../http.

## Local Contracts
- The CA is always user-supplied (PEM files, PKCS#12, or in-memory PEM bytes). Never auto-generate a CA.
- Use only the pure-Rust stack: `rustls` with the `ring` provider (`crypto_provider()`), `rcgen`, `x509-parser`, `p12-keystore`.
- Private keys and passphrases never appear in `Debug` output, logs, or error strings.
- Each leaf gets a fresh ECDSA P-256 key pair, a random 128-bit positive serial, `CN` = host, and one SAN. Validity is `now - 5m` to `now + TTL + 5m`.
- The leaf-certificate cache is the only cache in the engine. It is TTL-bound and checked lazily on every read (an expired leaf is never returned), single-flight per host, optionally capped, and reaped by a background task until cancelled.

## Feature Map
- **MITM state and options** — `MitmState`, `MitmOptions` (including `bypass_hosts` exclusion rules), `CaSource`, `UpstreamTlsVerification` (upstream certificate verification mode, default `Strict`), TTL and cache-size defaults, the shared `ring` crypto provider. Start: `mod.rs`. Files: `../http/mitm_tunnel.rs` (`upstream_client_config`, `MitmInterceptor::should_bypass`).
- **CA loading and validation** — PEM (PKCS#8, PKCS#1, SEC1, encrypted PKCS#8), PKCS#12, and in-memory bytes, with startup validation and conversion hints. Start: `ca.rs`.
- **Leaf signing** — per-host leaf certificates signed by the loaded CA. Start: `signer.rs`.
- **Leaf-certificate cache** — TTL cache with single-flight signing, entry cap, and reaper. Start: `cache.rs`.

## Child DOX Index
- (none)
