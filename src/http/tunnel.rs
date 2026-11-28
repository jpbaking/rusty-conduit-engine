//! Non-MITM HTTPS `CONNECT` tunnel over the Auto Switch router: an opaque TCP relay.
//!
//! The `CONNECT` authority is validated as `host:port` and routed with
//! `scheme = Some("https")` and no URL (the tunnel is opaque). The decision's failover chain
//! is dialed through the [`ProfileRegistry`](crate::upstream::ProfileRegistry) (fresh
//! connection; hostnames are sent unresolved to proxy upstreams), the client receives
//! `200 Connection Established`, and after `hyper::upgrade::on` the two streams are relayed
//! with [`relay_bidirectional`]. A `block` route or filter denial gets `403 Forbidden`. One
//! access record (`proto=https`, `mitm=false`, `method=CONNECT`, plus `rule` / `profile` /
//! `upstream`) is emitted when the tunnel closes.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::{
    connect_established, dial_error_status, failed_route, ConnectTarget, ProxyBody, RequestCtx,
};
use crate::net::relay_bidirectional;
use crate::router::{RouteDecision, RouteRequest};
use crate::routing::{Inbound, RoutingState};

/// Handle a `CONNECT` request without interception.
pub(crate) async fn handle(ctx: &RequestCtx, req: Request<Incoming>) -> Response<ProxyBody> {
    let authority = req
        .uri()
        .authority()
        .map(|a| a.as_str().to_string())
        .unwrap_or_default();
    let logger = ctx.proxy.logger.clone();

    let Some(target) = ConnectTarget::parse(&authority) else {
        let mut guard = ctx.access_guard("https", false, "CONNECT", authority.clone());
        guard.set_status(StatusCode::BAD_REQUEST);
        logger.debug(format!(
            "rejected CONNECT from {}: invalid target {authority:?}",
            ctx.peer
        ));
        return ctx.error(
            StatusCode::BAD_REQUEST,
            "CONNECT target must be host:port",
            None,
        );
    };

    let page = target.authority();
    let route_req = RouteRequest {
        inbound: Inbound::Http,
        host: &target.host,
        port: target.port,
        scheme: Some("https"),
        url: None,
    };
    let decision = match ctx.route.resolve(&route_req) {
        Ok(d) => d,
        Err(denied) => {
            let mut guard = ctx.access_guard("https", false, "CONNECT", authority);
            guard.set_status(StatusCode::FORBIDDEN);
            logger.info(format!(
                "CONNECT {target} from {} denied by filter: {denied}",
                ctx.peer
            ));
            return ctx.error(
                StatusCode::FORBIDDEN,
                "destination not allowed",
                Some(page.as_str()),
            );
        }
    };
    handle_resolved(ctx, req, target, decision).await
}

/// Handle a `CONNECT` request without interception using an already-parsed target and routing
/// decision (shared by plain `CONNECT` and MITM-bypassed rules).
pub(crate) async fn handle_resolved(
    ctx: &RequestCtx,
    req: Request<Incoming>,
    target: ConnectTarget,
    decision: RouteDecision,
) -> Response<ProxyBody> {
    let authority = target.authority();
    let mut guard = ctx.access_guard("https", false, "CONNECT", authority.clone());
    let logger = ctx.proxy.logger.clone();
    let page = Some(authority.as_str());
    let state = Arc::clone(&ctx.state);
    let rule = decision.rule_label().to_string();
    let chain = match state.registry.chain_for(&decision.targets) {
        Ok(c) => c.with_logger(logger.clone()),
        Err(e) => {
            let (profile, upstream) = failed_route(&state, &decision);
            guard.track(
                &state.counters,
                &rule,
                &RoutingState::counted_profile(&decision, None),
            );
            guard.set_route(&rule, profile, upstream);
            guard.set_status(StatusCode::BAD_GATEWAY);
            logger.error(format!("CONNECT {target}: {e}"));
            return ctx.error(StatusCode::BAD_GATEWAY, "upstream not configured", page);
        }
    };

    let outcome = match chain
        .dial(&target.host, target.port, state.connect_timeout)
        .await
    {
        Ok(o) => o,
        Err(e) => {
            let status = dial_error_status(&e);
            let (profile, upstream) = failed_route(&state, &decision);
            if status == StatusCode::FORBIDDEN {
                logger.info(format!(
                    "CONNECT {target} blocked: rule={rule} profile={profile} upstream={upstream}"
                ));
            } else {
                logger.error(format!(
                    "CONNECT {target} failed: rule={rule} profile={profile} \
                     upstream={upstream} error={e}"
                ));
            }
            guard.track(
                &state.counters,
                &rule,
                &RoutingState::counted_profile(&decision, None),
            );
            guard.set_route(&rule, profile, upstream);
            guard.set_status(status);
            return ctx.error(status, "upstream connection failed", page);
        }
    };
    let profile = decision
        .targets
        .get(outcome.attempt)
        .map(|t| t.label())
        .unwrap_or_else(|| outcome.profile.clone());
    logger.debug(format!(
        "CONNECT {target} from {}: rule={rule} profile={profile} upstream={}",
        ctx.peer, outcome.upstream
    ));
    guard.track(
        &state.counters,
        &rule,
        &RoutingState::counted_profile(&decision, Some(outcome.attempt)),
    );
    guard.set_route(&rule, profile, outcome.upstream);
    let mut upstream = outcome.stream;

    guard.set_status(StatusCode::OK);
    let shutdown = ctx.shutdown.clone();
    let peer = ctx.peer;
    ctx.tasks.spawn(async move {
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(u) => u,
            Err(e) => {
                logger.debug(format!("CONNECT upgrade from {peer} failed: {e}"));
                return;
            }
        };
        let mut client =
            CountingStream::new(TokioIo::new(upgraded), guard.bytes_in(), guard.bytes_out());
        tokio::select! {
            res = relay_bidirectional(&mut client, &mut upstream) => {
                if let Err(e) = res {
                    logger.debug(format!("CONNECT tunnel {peer} -> {target} ended: {e}"));
                }
            }
            () = shutdown.cancelled() => {
                logger.debug(format!("CONNECT tunnel {peer} -> {target} closed by shutdown"));
            }
        }
        drop(guard);
    });
    connect_established()
}

/// Wraps the client side of a tunnel, counting bytes read from (`bytes_in`) and written to
/// (`bytes_out`) the client so totals survive errors and shutdown.
pub struct CountingStream<S> {
    inner: S,
    read: Arc<AtomicU64>,
    written: Arc<AtomicU64>,
}

impl<S> CountingStream<S> {
    pub fn new(inner: S, read: Arc<AtomicU64>, written: Arc<AtomicU64>) -> Self {
        Self {
            inner,
            read,
            written,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CountingStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        ready!(Pin::new(&mut self.inner).poll_read(cx, buf))?;
        let n = buf.filled().len() - before;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountingStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let n = ready!(Pin::new(&mut self.inner).poll_write(cx, buf))?;
        self.written.fetch_add(n as u64, Ordering::Relaxed);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
