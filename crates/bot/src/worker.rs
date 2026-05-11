//! Delivery worker: claims due alerts and pushes them to Telegram.
//!
//! The loop:
//!  1. Compute next wake-up = `MIN(fire_at) WHERE state = pending`, or far
//!     future if there are no pending alerts.
//!  2. Wait for whichever happens first: that timestamp, a `NOTIFY` from the
//!     DB (new alert / cancellation), or shutdown.
//!  3. Atomically claim up to N due alerts via `FOR UPDATE SKIP LOCKED`.
//!  4. For each: send via Bot API, mark sent. On 429 reschedule, on 4xx other
//!     than 429 mark failed, on transient error retry up to MAX_ATTEMPTS.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgListener;
use teloxide::prelude::*;
use teloxide::ApiError;
use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use botcore::{Alert, Language, Recurrence};
use storage::PgStore;

use crate::messages as m;
use crate::render;

const MAX_ATTEMPTS: i16 = 5;
const STALE_CLAIM_SECS: i64 = 60;
const REAPER_INTERVAL: Duration = Duration::from_secs(30);
const CLAIM_BATCH: i64 = 100;
/// A delivery is "delayed" if it fires more than this far behind its
/// scheduled time. Below this we treat it as normal latency.
const DELAYED_THRESHOLD_SECS: i64 = 60;

pub async fn run(bot: Bot, store: Arc<PgStore>, shutdown: Arc<Notify>) -> anyhow::Result<()> {
    let mut listener = PgListener::connect_with(store.pool()).await?;
    listener.listen("alerts_changed").await?;
    info!("worker listening on `alerts_changed`");

    let reaper_store = store.clone();
    let reaper_shutdown = shutdown.clone();
    tokio::spawn(async move {
        reaper_loop(reaper_store, reaper_shutdown).await;
    });

    loop {
        let next_wake = compute_next_wake(&store).await;

        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                info!("worker shutting down");
                return Ok(());
            }
            _ = listener.recv() => {
                debug!("worker woke on NOTIFY");
            }
            _ = sleep_until(next_wake) => {
                debug!("worker woke on timer");
            }
        }

        if let Err(e) = process_due(&bot, &store).await {
            error!(error = ?e, "process_due failed");
        }
    }
}

async fn process_due(bot: &Bot, store: &PgStore) -> anyhow::Result<()> {
    let claimed = store.claim_due_alerts(CLAIM_BATCH).await?;
    for alert in claimed {
        deliver(bot, store, alert).await;
    }
    Ok(())
}

async fn deliver(bot: &Bot, store: &PgStore, alert: Alert) {
    // Look up creator language for delay-prefix localisation. Failure to
    // find the user is non-fatal — fall back to German (the bot's default).
    let lang = match store.get_user(alert.user_id).await {
        Ok(Some(u)) => u.language,
        _ => Language::De,
    };

    let now = Utc::now();
    let delay_secs = (now - alert.fire_at).num_seconds();
    let body = if delay_secs > DELAYED_THRESHOLD_SECS {
        let human = render::format_duration_human(delay_secs);
        format!("{}{}", m::delayed_prefix(lang, &human), alert.text)
    } else {
        alert.text.clone()
    };

    let send = bot.send_message(ChatId(alert.chat_id), &body).send().await;

    match send {
        Ok(_) => {
            if let Err(e) = finalise_after_send(store, &alert).await {
                error!(id = alert.id, error = ?e, "finalise_after_send failed");
            }
        }
        Err(teloxide::RequestError::RetryAfter(seconds)) => {
            // Telegram global/group rate limit. Reschedule and retry.
            let new_fire = Utc::now() + chrono::Duration::seconds(seconds.seconds() as i64 + 1);
            warn!(id = alert.id, retry_in = seconds.seconds(), "telegram 429, rescheduling");
            if let Err(e) = store.reschedule(alert.id, new_fire).await {
                error!(id = alert.id, error = ?e, "reschedule failed");
            }
        }
        Err(teloxide::RequestError::Api(ApiError::BotBlocked))
        | Err(teloxide::RequestError::Api(ApiError::ChatNotFound))
        | Err(teloxide::RequestError::Api(ApiError::UserDeactivated)) => {
            // Permanent: nobody to deliver to.
            warn!(id = alert.id, "permanent delivery failure");
            if let Err(e) = store
                .mark_failed(alert.id, "permanent: delivery target unreachable")
                .await
            {
                error!(id = alert.id, error = ?e, "mark_failed failed");
            }
        }
        Err(e) => {
            // Transient — retry with exponential backoff up to MAX_ATTEMPTS.
            if alert.attempts >= MAX_ATTEMPTS {
                let msg = format!("max attempts: {e}");
                warn!(id = alert.id, attempts = alert.attempts, "giving up");
                if let Err(e) = store.mark_failed(alert.id, &msg).await {
                    error!(id = alert.id, error = ?e, "mark_failed failed");
                }
            } else {
                let backoff = 5_i64 << alert.attempts.min(6);
                let new_fire = Utc::now() + chrono::Duration::seconds(backoff);
                warn!(id = alert.id, attempt = alert.attempts, backoff, "transient error, retrying");
                if let Err(e) = store.reschedule(alert.id, new_fire).await {
                    error!(id = alert.id, error = ?e, "reschedule failed");
                }
            }
        }
    }
}

/// On successful delivery: if recurring, compute the next fire time relative
/// to *now* (skipping any missed occurrences during downtime so we don't fire
/// the past N intervals back-to-back) and reschedule. Otherwise mark sent.
async fn finalise_after_send(store: &PgStore, alert: &Alert) -> anyhow::Result<()> {
    let Some(rec) = alert.recurrence.as_ref() else {
        store.mark_sent(alert.id).await?;
        return Ok(());
    };

    // Look up creator timezone so weekly/monthly/yearly resolve correctly.
    let tz = store
        .get_user(alert.user_id)
        .await?
        .map(|u| u.timezone)
        .unwrap_or(chrono_tz::UTC);

    let now = Utc::now();
    match rec.next_after(now, tz) {
        Some(next) => store.reschedule(alert.id, next).await?,
        None => store.mark_sent(alert.id).await?,
    }
    Ok(())
}

async fn compute_next_wake(store: &PgStore) -> DateTime<Utc> {
    match store.next_pending_fire_at().await {
        Ok(Some(t)) => t,
        Ok(None) => Utc::now() + chrono::Duration::hours(1),
        Err(e) => {
            error!(error = ?e, "next_pending_fire_at failed");
            Utc::now() + chrono::Duration::seconds(30)
        }
    }
}

async fn sleep_until(target: DateTime<Utc>) {
    let now = Utc::now();
    if target <= now {
        return;
    }
    let delta = (target - now).to_std().unwrap_or(Duration::from_secs(0));
    sleep(delta).await;
}

async fn reaper_loop(store: Arc<PgStore>, shutdown: Arc<Notify>) {
    loop {
        tokio::select! {
            _ = shutdown.notified() => return,
            _ = sleep(REAPER_INTERVAL) => {}
        }

        match store.release_stale_claims(STALE_CLAIM_SECS).await {
            Ok(n) if n > 0 => warn!(reaped = n, "released stale claims"),
            Ok(_) => {}
            Err(e) => error!(error = ?e, "reaper failed"),
        }
        match store.purge_old_updates(86_400).await {
            Ok(_) => {}
            Err(e) => error!(error = ?e, "purge_old_updates failed"),
        }
    }
}
