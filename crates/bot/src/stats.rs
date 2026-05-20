//! Read-only HTTP stats endpoint for external dashboards (e.g. Homepage).
//!
//! Deliberately a *separate* axum server on its own port rather than bolted
//! onto teloxide's webhook listener:
//!   * the Telegram hot path stays untouched (zero blast radius there),
//!   * it runs in both webhook and polling mode, and
//!   * it binds a LAN-only port that isn't fronted by the public cloudflared
//!     tunnel, so the counts aren't exposed to the internet.
//!
//! Opt-in: only starts when `STATS_LISTEN` is set (see `Config::from_env`).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use tokio::sync::Notify;
use tracing::info;

use storage::PgStore;

/// Serve `GET /stats` until `shutdown` is notified.
pub async fn run(store: Arc<PgStore>, listen: SocketAddr, shutdown: Arc<Notify>) -> Result<()> {
    let app = Router::new()
        .route("/stats", get(stats_handler))
        .with_state(store);

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding stats listener on {listen}"))?;
    info!(%listen, "stats endpoint listening on /stats");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.notified().await })
        .await
        .context("stats server error")?;
    Ok(())
}

/// JSON snapshot of a few aggregate counts. Hand-built (integers only, nothing
/// to escape) to avoid pulling serde into this crate just for four numbers.
async fn stats_handler(State(store): State<Arc<PgStore>>) -> impl IntoResponse {
    match store.stats().await {
        Ok(s) => {
            let body = format!(
                r#"{{"users":{},"alerts_active":{},"alerts_sent":{},"alerts_total":{}}}"#,
                s.users, s.active, s.sent, s.total
            );
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(error = ?e, "stats query failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "stats unavailable").into_response()
        }
    }
}
