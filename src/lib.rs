//! `rusty-conduit-engine`: the shared proxy and routing engine behind the
//! rusty-conduit CLI family (`rusty-conduit`, `rusty-conduit-ssh`,
//! `rusty-conduit-switch`).
//!
//! The crate uses a pure-Rust TLS/crypto stack (`rustls` with the `ring`
//! provider); OpenSSL and `aws-lc` are never linked.
//!
//! Modules: the routing/profile model and compiled snapshots ([`routing`]), rule matching
//! ([`router`]), upstream dialers ([`upstream`]), the HTTP/HTTPS ([`http`]) and
//! SOCKS4/4a/5 ([`socks`]) protocol engines, HTTPS MITM ([`mitm`]), listeners and
//! multiplexing ([`server`], [`mux`]), destination filtering ([`filter`]), socket helpers
//! ([`net`]), and logging ([`logging`]). Configuration-file loading stays in the CLIs.
//!
//! The default-on `routing` cargo feature provides regex-based rule matching. Without it, every
//! module still builds, and rules that use regex-based conditions fail to compile with an error
//! that names the feature.

pub mod filter;
pub mod http;
pub mod logging;
pub mod mitm;
pub mod mux;
pub mod net;
pub mod router;
pub mod routing;
pub mod server;
pub mod socks;
pub mod upstream;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(env!("CARGO_PKG_NAME"), "rusty-conduit-engine");
    }
}
