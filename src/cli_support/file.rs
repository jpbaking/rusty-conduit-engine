//! Config-file sections the CLIs share: the `[mitm]` and `[logging]` tables, and their merge
//! with command-line values (command line > file > default).
//!
//! The tables reject unknown keys, so a CLI embeds them as named fields
//! (`mitm: Option<FileMitmConfig>`, `logging: Option<FileLoggingConfig>`) rather than with
//! `#[serde(flatten)]`, which cannot be combined with `deny_unknown_fields`. Top-level keys
//! (keepalive, `allow` / `block`) stay in each CLI. Nothing here reads files: the CLI parses its
//! TOML and passes the tables in.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::logging::{LogFormat, LogLevel};
use crate::mitm::{DEFAULT_MITM_CERT_CACHE_MAX, DEFAULT_MITM_CERT_TTL};

/// Notice emitted when a file still uses the deprecated `mitm.cert_cache_max` key.
pub const DEPRECATED_CERT_CACHE_MAX_NOTICE: &str =
    "NOTICE: TOML mitm.cert_cache_max is deprecated; use mitm.cert_cache_max_entries";

/// A config-file value that could not be parsed. Its message matches the CLIs' former
/// `ConfigError::InvalidFileValue`, so a CLI can map it field for field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid value for `{key}` in config file: {message}")]
pub struct FileValueError {
    /// Dotted key, for example `mitm.cert_ttl`.
    pub key: &'static str,
    pub message: String,
}

/// Parse an optional human-readable duration (`30m`, `1h`, `3600s`) found under `key`.
pub fn file_duration(
    key: &'static str,
    raw: Option<&str>,
) -> Result<Option<Duration>, FileValueError> {
    raw.map(|s| {
        humantime::parse_duration(s).map_err(|e| FileValueError {
            key,
            message: format!("{s:?}: {e}"),
        })
    })
    .transpose()
}

/// `[mitm]` table of a CLI config file.
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMitmConfig {
    pub ca_cert: Option<PathBuf>,
    pub ca_key: Option<PathBuf>,
    pub ca_p12: Option<PathBuf>,
    pub ca_passphrase: Option<String>,
    pub ca_passphrase_file: Option<PathBuf>,
    /// Human-readable duration such as `30m`, `1h`, or `3600s`.
    pub cert_ttl: Option<String>,
    pub cert_cache_max_entries: Option<usize>,
    /// Deprecated spelling of `cert_cache_max_entries` (used only when that key is absent).
    pub cert_cache_max: Option<usize>,
    /// Destinations (IP, CIDR, FQDN, or `*.domain`) whose upstream TLS certificates are not
    /// verified.
    #[serde(default)]
    pub insecure_hosts: Vec<String>,
    /// Blind trust: skip upstream TLS certificate verification for every MITM target (unsafe).
    pub insecure: Option<bool>,
    /// Destinations (IP, CIDR, FQDN, or `*.domain`) excluded from MITM interception.
    #[serde(default)]
    pub bypass_hosts: Vec<String>,
}

impl fmt::Debug for FileMitmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileMitmConfig")
            .field("ca_cert", &self.ca_cert)
            .field("ca_key", &self.ca_key)
            .field("ca_p12", &self.ca_p12)
            .field(
                "ca_passphrase",
                &self.ca_passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("ca_passphrase_file", &self.ca_passphrase_file)
            .field("cert_ttl", &self.cert_ttl)
            .field("cert_cache_max_entries", &self.cert_cache_max_entries)
            .field("cert_cache_max", &self.cert_cache_max)
            .field("insecure_hosts", &self.insecure_hosts)
            .field("insecure", &self.insecure)
            .field("bypass_hosts", &self.bypass_hosts)
            .finish()
    }
}

/// `[logging]` table of a CLI config file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileLoggingConfig {
    pub level: Option<String>,
    pub format: Option<String>,
    pub http_access_log: Option<PathBuf>,
    pub socks_log: Option<PathBuf>,
}

/// MITM values from the command line or environment, before the file is merged. Empty lists
/// and `None` mean "not given".
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MitmCliValues {
    pub ca_cert: Option<PathBuf>,
    pub ca_key: Option<PathBuf>,
    pub ca_p12: Option<PathBuf>,
    pub ca_passphrase: Option<String>,
    pub ca_passphrase_file: Option<PathBuf>,
    pub cert_ttl: Option<Duration>,
    pub cert_cache_max_entries: Option<usize>,
    pub insecure_hosts: Vec<String>,
    pub bypass_hosts: Vec<String>,
    pub insecure: bool,
}

impl fmt::Debug for MitmCliValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MitmCliValues")
            .field("ca_cert", &self.ca_cert)
            .field("ca_key", &self.ca_key)
            .field("ca_p12", &self.ca_p12)
            .field(
                "ca_passphrase",
                &self.ca_passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("ca_passphrase_file", &self.ca_passphrase_file)
            .field("cert_ttl", &self.cert_ttl)
            .field("cert_cache_max_entries", &self.cert_cache_max_entries)
            .field("insecure_hosts", &self.insecure_hosts)
            .field("bypass_hosts", &self.bypass_hosts)
            .field("insecure", &self.insecure)
            .finish()
    }
}

/// Merged MITM settings, plus any deprecation notices for the CLI to print.
#[derive(Clone, PartialEq, Eq)]
pub struct MitmSettings {
    pub ca_cert: Option<PathBuf>,
    pub ca_key: Option<PathBuf>,
    pub ca_p12: Option<PathBuf>,
    pub ca_passphrase: Option<String>,
    pub ca_passphrase_file: Option<PathBuf>,
    pub cert_ttl: Duration,
    pub cert_cache_max_entries: usize,
    pub insecure_hosts: Vec<String>,
    pub bypass_hosts: Vec<String>,
    pub insecure: bool,
    /// For example [`DEPRECATED_CERT_CACHE_MAX_NOTICE`].
    pub notices: Vec<String>,
}

impl fmt::Debug for MitmSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MitmSettings")
            .field("ca_cert", &self.ca_cert)
            .field("ca_key", &self.ca_key)
            .field("ca_p12", &self.ca_p12)
            .field(
                "ca_passphrase",
                &self.ca_passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("ca_passphrase_file", &self.ca_passphrase_file)
            .field("cert_ttl", &self.cert_ttl)
            .field("cert_cache_max_entries", &self.cert_cache_max_entries)
            .field("insecure_hosts", &self.insecure_hosts)
            .field("bypass_hosts", &self.bypass_hosts)
            .field("insecure", &self.insecure)
            .field("notices", &self.notices)
            .finish()
    }
}

/// Merge `[mitm]` with the command line: each value comes from the command line, else the
/// file, else the engine default. Host lists come from the command line when it gives any,
/// else from the file. `insecure` is on when either side turns it on. The cache size is
/// `cert_cache_max_entries` from the command line, then the file, then the deprecated file key
/// `cert_cache_max`; using that key adds [`DEPRECATED_CERT_CACHE_MAX_NOTICE`].
pub fn merge_mitm(
    cli: MitmCliValues,
    file: Option<FileMitmConfig>,
) -> Result<MitmSettings, FileValueError> {
    let file = file.unwrap_or_default();
    let file_ttl = file_duration("mitm.cert_ttl", file.cert_ttl.as_deref())?;
    let mut notices = Vec::new();
    if file.cert_cache_max.is_some() {
        notices.push(DEPRECATED_CERT_CACHE_MAX_NOTICE.to_string());
    }
    let pick = |cli: Vec<String>, file: Vec<String>| if cli.is_empty() { file } else { cli };
    Ok(MitmSettings {
        ca_cert: cli.ca_cert.or(file.ca_cert),
        ca_key: cli.ca_key.or(file.ca_key),
        ca_p12: cli.ca_p12.or(file.ca_p12),
        ca_passphrase: cli.ca_passphrase.or(file.ca_passphrase),
        ca_passphrase_file: cli.ca_passphrase_file.or(file.ca_passphrase_file),
        cert_ttl: cli.cert_ttl.or(file_ttl).unwrap_or(DEFAULT_MITM_CERT_TTL),
        cert_cache_max_entries: cli
            .cert_cache_max_entries
            .or(file.cert_cache_max_entries)
            .or(file.cert_cache_max)
            .unwrap_or(DEFAULT_MITM_CERT_CACHE_MAX),
        insecure_hosts: pick(cli.insecure_hosts, file.insecure_hosts),
        bypass_hosts: pick(cli.bypass_hosts, file.bypass_hosts),
        insecure: cli.insecure || file.insecure.unwrap_or(false),
        notices,
    })
}

/// Logging values from the command line or environment, before the file is merged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoggingCliValues {
    pub level: Option<LogLevel>,
    pub format: Option<LogFormat>,
    pub http_access_log: Option<PathBuf>,
    pub socks_log: Option<PathBuf>,
}

/// Merged logging settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingSettings {
    pub level: LogLevel,
    pub format: LogFormat,
    pub http_access_log: Option<PathBuf>,
    pub socks_log: Option<PathBuf>,
}

/// Merge `[logging]` with the command line (command line > file > `INFO` / `text`).
pub fn merge_logging(
    cli: LoggingCliValues,
    file: Option<FileLoggingConfig>,
) -> Result<LoggingSettings, FileValueError> {
    let file = file.unwrap_or_default();
    let level = file
        .level
        .as_deref()
        .map(|s| {
            s.parse::<LogLevel>().map_err(|message| FileValueError {
                key: "logging.level",
                message,
            })
        })
        .transpose()?;
    let format = file
        .format
        .as_deref()
        .map(|s| {
            s.parse::<LogFormat>().map_err(|message| FileValueError {
                key: "logging.format",
                message,
            })
        })
        .transpose()?;
    Ok(LoggingSettings {
        level: cli.level.or(level).unwrap_or(LogLevel::Info),
        format: cli.format.or(format).unwrap_or(LogFormat::Text),
        http_access_log: cli.http_access_log.or(file.http_access_log),
        socks_log: cli.socks_log.or(file.socks_log),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        mitm: Option<FileMitmConfig>,
        logging: Option<FileLoggingConfig>,
    }

    const FULL: &str = r#"
[mitm]
ca_cert = "/etc/rc/ca.pem"
ca_key = "/etc/rc/ca.key"
ca_p12 = "/etc/rc/ca.p12"
ca_passphrase = "file-secret"
ca_passphrase_file = "/run/secrets/pass"
cert_ttl = "30m"
cert_cache_max_entries = 500
insecure_hosts = ["self-signed.lan", "10.0.0.0/8"]
insecure = true
bypass_hosts = ["*.bank.example"]

[logging]
level = "debug"
format = "json"
http_access_log = "/var/log/rc/http.log"
socks_log = "/var/log/rc/socks.log"
"#;

    fn full() -> File {
        toml::from_str(FULL).unwrap()
    }

    #[test]
    fn sections_parse_every_key_and_reject_unknown_ones() {
        let f = full();
        let m = f.mitm.unwrap();
        assert_eq!(m.cert_cache_max_entries, Some(500));
        assert_eq!(m.bypass_hosts, ["*.bank.example"]);
        assert_eq!(f.logging.unwrap().format.as_deref(), Some("json"));
        let switch_legacy: File = toml::from_str("[mitm]\ncert_cache_max = 9\n").unwrap();
        assert_eq!(switch_legacy.mitm.unwrap().cert_cache_max, Some(9));
        assert!(toml::from_str::<File>("[mitm]\nca_certs = \"x\"\n").is_err());
        assert!(toml::from_str::<File>("[logging]\nlevels = \"x\"\n").is_err());
    }

    #[test]
    fn mitm_defaults_when_nothing_is_given() {
        let s = merge_mitm(MitmCliValues::default(), None).unwrap();
        assert_eq!(s.cert_ttl, DEFAULT_MITM_CERT_TTL);
        assert_eq!(s.cert_cache_max_entries, DEFAULT_MITM_CERT_CACHE_MAX);
        assert!(s.ca_cert.is_none() && s.ca_passphrase.is_none());
        assert!(s.insecure_hosts.is_empty() && s.bypass_hosts.is_empty() && !s.insecure);
        assert!(s.notices.is_empty());
    }

    #[test]
    fn mitm_file_fills_what_the_command_line_leaves_out() {
        let s = merge_mitm(MitmCliValues::default(), full().mitm).unwrap();
        assert_eq!(s.ca_cert, Some(PathBuf::from("/etc/rc/ca.pem")));
        assert_eq!(s.ca_key, Some(PathBuf::from("/etc/rc/ca.key")));
        assert_eq!(s.ca_p12, Some(PathBuf::from("/etc/rc/ca.p12")));
        assert_eq!(s.ca_passphrase.as_deref(), Some("file-secret"));
        assert_eq!(
            s.ca_passphrase_file,
            Some(PathBuf::from("/run/secrets/pass"))
        );
        assert_eq!(s.cert_ttl, Duration::from_secs(30 * 60));
        assert_eq!(s.cert_cache_max_entries, 500);
        assert_eq!(s.insecure_hosts, ["self-signed.lan", "10.0.0.0/8"]);
        assert_eq!(s.bypass_hosts, ["*.bank.example"]);
        assert!(s.insecure);
    }

    #[test]
    fn mitm_command_line_wins_every_field() {
        let cli = MitmCliValues {
            ca_cert: Some("/cli/ca.pem".into()),
            ca_key: Some("/cli/ca.key".into()),
            ca_p12: Some("/cli/ca.p12".into()),
            ca_passphrase: Some("cli-secret".into()),
            ca_passphrase_file: Some("/cli/pass".into()),
            cert_ttl: Some(Duration::from_secs(90)),
            cert_cache_max_entries: Some(7),
            insecure_hosts: vec!["cli.lan".into()],
            bypass_hosts: vec!["cli.bank".into()],
            insecure: false,
        };
        let s = merge_mitm(cli, full().mitm).unwrap();
        assert_eq!(s.ca_cert, Some(PathBuf::from("/cli/ca.pem")));
        assert_eq!(s.ca_key, Some(PathBuf::from("/cli/ca.key")));
        assert_eq!(s.ca_p12, Some(PathBuf::from("/cli/ca.p12")));
        assert_eq!(s.ca_passphrase.as_deref(), Some("cli-secret"));
        assert_eq!(s.ca_passphrase_file, Some(PathBuf::from("/cli/pass")));
        assert_eq!(s.cert_ttl, Duration::from_secs(90));
        assert_eq!(s.cert_cache_max_entries, 7);
        assert_eq!(s.insecure_hosts, ["cli.lan"]);
        assert_eq!(s.bypass_hosts, ["cli.bank"]);
        // `insecure` is an OR: the file's `true` still applies when the flag is off.
        assert!(s.insecure);
        let only_cli = merge_mitm(
            MitmCliValues {
                insecure: true,
                ..MitmCliValues::default()
            },
            None,
        );
        assert!(only_cli.unwrap().insecure);
    }

    #[test]
    fn deprecated_cache_key_has_lowest_precedence_and_a_notice() {
        let legacy = || {
            Some(FileMitmConfig {
                cert_cache_max: Some(9),
                ..FileMitmConfig::default()
            })
        };
        let s = merge_mitm(MitmCliValues::default(), legacy()).unwrap();
        assert_eq!(s.cert_cache_max_entries, 9);
        assert_eq!(s.notices, [DEPRECATED_CERT_CACHE_MAX_NOTICE]);

        let both = FileMitmConfig {
            cert_cache_max_entries: Some(20),
            cert_cache_max: Some(9),
            ..FileMitmConfig::default()
        };
        let s = merge_mitm(MitmCliValues::default(), Some(both)).unwrap();
        assert_eq!(s.cert_cache_max_entries, 20);
        assert_eq!(
            s.notices.len(),
            1,
            "the key is still reported when overridden"
        );

        let cli = MitmCliValues {
            cert_cache_max_entries: Some(3),
            ..MitmCliValues::default()
        };
        assert_eq!(merge_mitm(cli, legacy()).unwrap().cert_cache_max_entries, 3);
    }

    #[test]
    fn bad_file_values_name_their_key() {
        let file = FileMitmConfig {
            cert_ttl: Some("soon".into()),
            ..FileMitmConfig::default()
        };
        let err = merge_mitm(MitmCliValues::default(), Some(file)).unwrap_err();
        assert_eq!(err.key, "mitm.cert_ttl");
        assert!(
            err.to_string()
                .starts_with("invalid value for `mitm.cert_ttl` in config file: \"soon\": "),
            "{err}"
        );

        let level = FileLoggingConfig {
            level: Some("loud".into()),
            ..FileLoggingConfig::default()
        };
        assert_eq!(
            merge_logging(LoggingCliValues::default(), Some(level))
                .unwrap_err()
                .key,
            "logging.level"
        );
        let format = FileLoggingConfig {
            format: Some("xml".into()),
            ..FileLoggingConfig::default()
        };
        assert_eq!(
            merge_logging(LoggingCliValues::default(), Some(format))
                .unwrap_err()
                .key,
            "logging.format"
        );
        assert_eq!(file_duration("k", None), Ok(None));
    }

    #[test]
    fn logging_precedence() {
        let d = merge_logging(LoggingCliValues::default(), None).unwrap();
        assert_eq!((d.level, d.format), (LogLevel::Info, LogFormat::Text));
        assert!(d.http_access_log.is_none() && d.socks_log.is_none());

        let f = merge_logging(LoggingCliValues::default(), full().logging).unwrap();
        assert_eq!((f.level, f.format), (LogLevel::Debug, LogFormat::Json));
        assert_eq!(
            f.http_access_log,
            Some(PathBuf::from("/var/log/rc/http.log"))
        );
        assert_eq!(f.socks_log, Some(PathBuf::from("/var/log/rc/socks.log")));

        let cli = LoggingCliValues {
            level: Some(LogLevel::Error),
            format: Some(LogFormat::Text),
            http_access_log: Some("/cli/http.log".into()),
            socks_log: Some("/cli/socks.log".into()),
        };
        let c = merge_logging(cli, full().logging).unwrap();
        assert_eq!((c.level, c.format), (LogLevel::Error, LogFormat::Text));
        assert_eq!(c.http_access_log, Some(PathBuf::from("/cli/http.log")));
        assert_eq!(c.socks_log, Some(PathBuf::from("/cli/socks.log")));
    }

    #[test]
    fn debug_never_shows_the_passphrase() {
        let file = full().mitm.unwrap();
        let cli = MitmCliValues {
            ca_passphrase: Some("cli-secret".into()),
            ..MitmCliValues::default()
        };
        let merged = merge_mitm(cli.clone(), Some(file.clone())).unwrap();
        for rendered in [
            format!("{file:?}"),
            format!("{cli:?}"),
            format!("{merged:?}"),
        ] {
            assert!(
                !rendered.contains("file-secret") && !rendered.contains("cli-secret"),
                "{rendered}"
            );
            assert!(rendered.contains("<redacted>"), "{rendered}");
        }
    }
}
