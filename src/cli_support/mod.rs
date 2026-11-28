//! Support code shared by the rusty-conduit CLIs, behind the opt-in `cli-support` feature:
//! open-file limit raising, startup path preflight, the shutdown signal, the `app_version!`
//! macro, and shared config-file section types. Default builds never include it.

pub mod fdlimit;
pub mod file;
pub mod preflight;

/// Resolve when the process receives `Ctrl+C` (all platforms) or `SIGTERM` (Unix).
///
/// If a signal handler cannot be installed the error is logged and that source is ignored.
pub async fn shutdown_signal(logger: &crate::logging::Logger) {
    let ctrl_c = async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => logger.info("received Ctrl+C"),
            Err(e) => {
                logger.error(format!("failed to listen for Ctrl+C: {e}"));
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
                logger.info("received SIGTERM");
            }
            Err(e) => {
                logger.error(format!("failed to listen for SIGTERM: {e}"));
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// The version `--version` and the startup banner report.
///
/// Release builds set `CONDUIT_RELEASE_VERSION` at compile time to the tag without its
/// leading `v` (pre-release suffixes such as `-beta.1` included); any other build reports
/// the `Cargo.toml` version with a `-dev` suffix.
///
/// Expands in the calling crate so `CONDUIT_RELEASE_VERSION` and `CARGO_PKG_VERSION`
/// reflect the calling crate rather than `rusty-conduit-engine`.
#[macro_export]
macro_rules! app_version {
    () => {
        match option_env!("CONDUIT_RELEASE_VERSION") {
            Some(version) if !version.is_empty() => version,
            _ => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
        }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_version_defaults_to_dev_when_release_env_unset() {
        if option_env!("CONDUIT_RELEASE_VERSION").is_none() {
            assert_eq!(
                crate::app_version!(),
                concat!(env!("CARGO_PKG_VERSION"), "-dev")
            );
        }
    }
}
