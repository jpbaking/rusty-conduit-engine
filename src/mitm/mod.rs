//! HTTPS MITM support: CA loading ([`ca`]), per-host leaf signing ([`signer`]), and the
//! TTL-bounded leaf certificate cache ([`cache`]) — the only cache in the engine. The
//! interception itself lives in [`crate::http::mitm_tunnel`].

pub mod ca;
pub mod cache;
pub mod signer;

use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustls::crypto::CryptoProvider;
use time::OffsetDateTime;

use crate::http::mitm_tunnel::{platform_client_config, MitmInterceptor};
use ca::{CaError, CaMaterial};
use cache::CertCache;
use signer::{LeafSigner, SignError};

/// The process-wide TLS crypto provider: `ring` (pure-Rust stack, no `aws-lc-rs`).
///
/// Passed explicitly to every rustls config builder, so no global default provider has to
/// be installed.
pub fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Default lifetime of generated MITM leaf certificates.
pub const DEFAULT_MITM_CERT_TTL: Duration = Duration::from_secs(60 * 60);
/// Minimum accepted MITM leaf certificate lifetime.
pub const MIN_MITM_CERT_TTL: Duration = Duration::from_secs(10);
/// Default maximum number of cached MITM leaf certificates.
pub const DEFAULT_MITM_CERT_CACHE_MAX: usize = 4096;

/// Where the MITM CA comes from (only present when MITM mode is ON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaSource {
    Pem { cert: PathBuf, key: PathBuf },
    Pkcs12 { path: PathBuf },
}

/// MITM settings, independent of how a CLI loads its configuration.
#[derive(Clone, PartialEq, Eq)]
pub struct MitmOptions {
    /// The CA to sign leaf certificates with; `None` means MITM mode is OFF.
    pub ca: Option<CaSource>,
    /// CA passphrase given inline (takes precedence over `passphrase_file`).
    pub passphrase: Option<String>,
    /// File holding the CA passphrase (one trailing newline is trimmed).
    pub passphrase_file: Option<PathBuf>,
    /// Lifetime of generated leaf certificates.
    pub cert_ttl: Duration,
    /// Maximum cached leaf certificates (`0` = unlimited).
    pub cert_cache_max: usize,
}

impl Default for MitmOptions {
    fn default() -> Self {
        Self {
            ca: None,
            passphrase: None,
            passphrase_file: None,
            cert_ttl: DEFAULT_MITM_CERT_TTL,
            cert_cache_max: DEFAULT_MITM_CERT_CACHE_MAX,
        }
    }
}

impl fmt::Debug for MitmOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MitmOptions")
            .field("ca", &self.ca)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("passphrase_file", &self.passphrase_file)
            .field("cert_ttl", &self.cert_ttl)
            .field("cert_cache_max", &self.cert_cache_max)
            .finish()
    }
}

impl MitmOptions {
    /// Whether HTTPS MITM inspection is enabled (a CA source is configured).
    pub fn enabled(&self) -> bool {
        self.ca.is_some()
    }

    /// Resolve the CA passphrase from the inline value or the passphrase file.
    pub fn resolve_passphrase(&self) -> Result<Option<String>, PassphraseError> {
        if let Some(p) = &self.passphrase {
            return Ok(Some(p.clone()));
        }
        match &self.passphrase_file {
            Some(path) => {
                let text = fs::read_to_string(path).map_err(|source| PassphraseError {
                    path: path.clone(),
                    source,
                })?;
                Ok(Some(crate::routing::trim_newline(&text).to_string()))
            }
            None => Ok(None),
        }
    }
}

/// The CA passphrase file could not be read.
#[derive(Debug, thiserror::Error)]
#[error("failed to read passphrase file {path}: {source}")]
pub struct PassphraseError {
    pub path: PathBuf,
    #[source]
    pub source: std::io::Error,
}

/// Startup failures while enabling MITM mode (all are `FATAL`).
#[derive(Debug, thiserror::Error)]
pub enum MitmError {
    #[error(transparent)]
    Passphrase(#[from] PassphraseError),
    #[error(transparent)]
    Ca(#[from] CaError),
    #[error(transparent)]
    Signer(#[from] SignError),
    #[error("failed to initialise upstream TLS verification: {0}")]
    UpstreamTls(#[from] rustls::Error),
}

/// Everything MITM mode needs at runtime, built once at startup.
#[derive(Debug)]
pub struct MitmState {
    pub interceptor: MitmInterceptor,
    pub cache: Arc<CertCache>,
    pub ca_subject: String,
    pub ca_not_after: OffsetDateTime,
}

impl MitmState {
    /// Load and validate the configured CA and build the signer, cache, and interceptor
    /// (upstream TLS verified with the platform verifier). Returns `Ok(None)` when MITM is
    /// OFF (no CA configured).
    pub fn from_options(cfg: &MitmOptions) -> Result<Option<Self>, MitmError> {
        let Some(source) = cfg.ca.clone() else {
            return Ok(None);
        };
        let provider = crypto_provider();
        let upstream = platform_client_config(Arc::clone(&provider))?;
        Self::build(cfg, &source, provider, upstream).map(Some)
    }

    /// Like [`Self::from_options`] but with an explicit upstream TLS client config.
    pub fn build(
        cfg: &MitmOptions,
        source: &CaSource,
        provider: Arc<CryptoProvider>,
        upstream: rustls::ClientConfig,
    ) -> Result<Self, MitmError> {
        let passphrase = cfg.resolve_passphrase()?;
        let ca = CaMaterial::load(source, passphrase.as_deref())?;
        Self::from_material(cfg, &ca, provider, upstream)
    }

    /// Build MITM state from in-memory PEM CA material, never touching the filesystem for
    /// the CA (for embedders that keep CA material in a vault). `cert_chain_pem` holds the
    /// CA certificate (optionally followed by intermediates) and `key_pem` its private key;
    /// an encrypted key is decrypted with `opts.passphrase` (or `opts.passphrase_file`).
    /// `opts.ca` is ignored; `opts.cert_ttl` and `opts.cert_cache_max` apply as usual.
    /// Upstream TLS is verified with the platform verifier.
    pub fn from_pem_bytes(
        cert_chain_pem: &[u8],
        key_pem: &[u8],
        opts: &MitmOptions,
    ) -> Result<Self, MitmError> {
        let provider = crypto_provider();
        let upstream = platform_client_config(Arc::clone(&provider))?;
        Self::from_pem_bytes_with_upstream(cert_chain_pem, key_pem, opts, provider, upstream)
    }

    /// Like [`Self::from_pem_bytes`] but with an explicit upstream TLS client config.
    pub fn from_pem_bytes_with_upstream(
        cert_chain_pem: &[u8],
        key_pem: &[u8],
        opts: &MitmOptions,
        provider: Arc<CryptoProvider>,
        upstream: rustls::ClientConfig,
    ) -> Result<Self, MitmError> {
        let passphrase = opts.resolve_passphrase()?;
        let ca = CaMaterial::from_pem_bytes(cert_chain_pem, key_pem, passphrase.as_deref())?;
        Self::from_material(opts, &ca, provider, upstream)
    }

    /// Build the signer, cache, and interceptor around an already validated CA.
    fn from_material(
        cfg: &MitmOptions,
        ca: &CaMaterial,
        provider: Arc<CryptoProvider>,
        upstream: rustls::ClientConfig,
    ) -> Result<Self, MitmError> {
        let signer = LeafSigner::new(ca, cfg.cert_ttl, Arc::clone(&provider))?;
        let cache = Arc::new(
            CertCache::new(Arc::new(signer), cfg.cert_ttl).with_max_entries(cfg.cert_cache_max),
        );
        let interceptor = MitmInterceptor::new(Arc::clone(&cache), Arc::new(upstream), provider);
        Ok(Self {
            interceptor,
            cache,
            ca_subject: ca.subject().to_string(),
            ca_not_after: ca.not_after(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mitm::ca::test_support::{ca_with_validity, test_ca};

    use std::fs;
    use std::time::Duration;

    fn cfg_with(dir: &std::path::Path, ca: &ca::test_support::TestCa) -> MitmOptions {
        let (cert, key) = (dir.join("ca.pem"), dir.join("ca.key"));
        fs::write(&cert, &ca.cert_pem).unwrap();
        fs::write(&key, &ca.key_pem).unwrap();
        MitmOptions {
            ca: Some(CaSource::Pem { cert, key }),
            cert_ttl: Duration::from_secs(600),
            ..MitmOptions::default()
        }
    }

    #[test]
    fn mitm_off_without_ca() {
        assert!(MitmState::from_options(&MitmOptions::default())
            .unwrap()
            .is_none());
    }

    #[test]
    fn builds_state_from_pem_files_and_rejects_expired_ca() {
        let dir = std::env::temp_dir().join(format!("rc-mitm-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        let cfg = cfg_with(&dir, &test_ca());
        let state = MitmState::from_options(&cfg).unwrap().unwrap();
        assert!(state.ca_subject.contains("Rusty Conduit Test CA"));
        assert_eq!(state.cache.ttl(), Duration::from_secs(600));
        assert_eq!(state.cache.max_entries(), DEFAULT_MITM_CERT_CACHE_MAX);
        assert!(state.cache.is_empty());

        let mut capped = cfg_with(&dir, &test_ca());
        capped.cert_cache_max = 7;
        let state = MitmState::from_options(&capped).unwrap().unwrap();
        assert_eq!(state.cache.max_entries(), 7);

        let now = OffsetDateTime::now_utc();
        let expired =
            ca_with_validity(now - time::Duration::days(3), now - time::Duration::days(1));
        let cfg = cfg_with(&dir, &expired);
        let err = MitmState::from_options(&cfg).unwrap_err();
        assert!(matches!(err, MitmError::Ca(CaError::Expired(_))), "{err}");
        assert!(err.to_string().contains("expired"), "{err}");

        let mut cfg = cfg_with(&dir, &test_ca());
        cfg.passphrase_file = Some(dir.join("missing-pass"));
        assert!(matches!(
            MitmState::from_options(&cfg),
            Err(MitmError::Passphrase(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn builds_state_from_in_memory_pem_bytes() {
        let ca = test_ca();
        let opts = MitmOptions {
            cert_ttl: Duration::from_secs(300),
            cert_cache_max: 9,
            ..MitmOptions::default()
        };
        let state = MitmState::from_pem_bytes(ca.cert_pem.as_bytes(), ca.key_pem.as_bytes(), &opts)
            .unwrap();
        assert!(state.ca_subject.contains("Rusty Conduit Test CA"));
        assert_eq!(state.cache.ttl(), Duration::from_secs(300));
        assert_eq!(state.cache.max_entries(), 9);
        // The in-memory CA really signs leaves.
        state.cache.get_or_create("example.test").await.unwrap();
        assert_eq!(state.cache.len(), 1);

        // `opts.ca` is ignored by the bytes constructor.
        let with_source = MitmOptions {
            ca: Some(CaSource::Pkcs12 {
                path: "/nonexistent/ca.p12".into(),
            }),
            ..opts.clone()
        };
        MitmState::from_pem_bytes(ca.cert_pem.as_bytes(), ca.key_pem.as_bytes(), &with_source)
            .unwrap();
    }

    #[test]
    fn in_memory_pem_bytes_honour_passphrase_and_validity() {
        let (cert, key) = ca::test_support::encrypted_fixture();
        let opts = MitmOptions {
            passphrase: Some(ca::test_support::FIXTURE_PASS.into()),
            ..MitmOptions::default()
        };
        let state = MitmState::from_pem_bytes(cert.as_bytes(), key.as_bytes(), &opts).unwrap();
        assert!(state.ca_subject.contains("Fixture"), "{}", state.ca_subject);

        let err =
            MitmState::from_pem_bytes(cert.as_bytes(), key.as_bytes(), &MitmOptions::default())
                .unwrap_err();
        assert!(
            matches!(err, MitmError::Ca(CaError::PassphraseRequired)),
            "{err}"
        );
        let wrong = MitmOptions {
            passphrase: Some("wrong".into()),
            ..MitmOptions::default()
        };
        assert!(matches!(
            MitmState::from_pem_bytes(cert.as_bytes(), key.as_bytes(), &wrong),
            Err(MitmError::Ca(CaError::Decrypt(_)))
        ));

        let now = OffsetDateTime::now_utc();
        let expired =
            ca_with_validity(now - time::Duration::days(3), now - time::Duration::days(1));
        let err = MitmState::from_pem_bytes(
            expired.cert_pem.as_bytes(),
            expired.key_pem.as_bytes(),
            &MitmOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, MitmError::Ca(CaError::Expired(_))), "{err}");
    }
}
