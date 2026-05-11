use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use teloxide::dispatching::ShutdownToken;
use teloxide::update_listeners::webhooks;
use teloxide::prelude::*;
use teloxide::types::BotCommand;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use url::Url;

use storage::PgStore;

mod admin;
mod commands;
mod handlers;
mod messages;
mod render;
mod worker;

use admin::AdminNotifier;
use commands::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = Config::from_env()?;
    info!(?config.transport, "starting alert-bot");

    let store = Arc::new(PgStore::connect(&config.database_url, 10).await?);
    store.migrate().await?;
    info!("migrations applied");

    let bot = Bot::new(&config.bot_token);
    register_command_menus(&bot).await;

    let notifier = AdminNotifier::new(bot.clone(), config.admin_chat_id);
    let transport_label = match config.transport {
        Transport::Webhook { .. } => "webhook",
        Transport::Polling => "polling",
    };
    notifier
        .notify(&format!(
            "✅ <b>alert-bot v{VERSION}</b> gestartet · transport={transport_label}"
        ))
        .await;

    let shutdown = Arc::new(Notify::new());

    let worker_handle = {
        let bot = bot.clone();
        let store = store.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move { worker::run(bot, store, shutdown).await })
    };

    let handler = dptree::entry()
        // Telegram retries webhook deliveries on timeout — drop duplicates by
        // update_id. Errors are logged and we fall through (better to risk a
        // duplicate than to silently swallow the update).
        .filter_async(|update: Update, store: Arc<PgStore>| async move {
            match store.try_register_update(update.id.0 as i64).await {
                Ok(fresh) => fresh,
                Err(e) => {
                    tracing::error!(error = ?e, update_id = update.id.0, "try_register_update failed");
                    true
                }
            }
        })
        .branch(
            Update::filter_message()
                .filter_command::<Command>()
                .endpoint(handlers::dispatch),
        )
        .branch(Update::filter_callback_query().endpoint(handlers::callback_dispatch))
        .branch(Update::filter_message().endpoint(handlers::handle_unhandled));

    // No `.enable_ctrlc_handler()` — that one only listens for SIGINT.
    // We install our own handler below that covers both SIGTERM and SIGINT
    // and feeds the dispatcher's ShutdownToken on either.
    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .dependencies(dptree::deps![store.clone()])
        .build();

    spawn_signal_handler(
        shutdown.clone(),
        dispatcher.shutdown_token(),
        notifier.clone(),
    );

    match config.transport {
        Transport::Webhook {
            listen,
            url,
            secret,
        } => {
            let mut options = webhooks::Options::new(listen, url);
            if let Some(s) = secret {
                options = options.secret_token(s);
            }
            let listener = webhooks::axum(bot.clone(), options)
                .await
                .context("setting up webhook listener")?;
            dispatcher
                .dispatch_with_listener(
                    listener,
                    LoggingErrorHandler::with_custom_text("webhook listener error"),
                )
                .await;
        }
        Transport::Polling => {
            // Local-dev fallback. Requires no public endpoint.
            dispatcher.dispatch().await;
        }
    }

    shutdown.notify_waiters();
    match worker_handle.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            notifier
                .notify(&format!("💥 <b>worker exited with error</b>\n{e}"))
                .await;
        }
        Err(join_err) => {
            notifier
                .notify(&format!("💥 <b>worker panicked</b>\n{join_err}"))
                .await;
        }
    }
    notifier.notify("🛑 alert-bot stopped").await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Config {
    bot_token: String,
    database_url: String,
    transport: Transport,
    admin_chat_id: Option<i64>,
}

#[derive(Debug)]
enum Transport {
    Webhook {
        listen: SocketAddr,
        url: Url,
        secret: Option<String>,
    },
    Polling,
}

impl Config {
    fn from_env() -> Result<Self> {
        let bot_token = std::env::var("BOT_TOKEN").context("BOT_TOKEN is required")?;
        let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;

        let transport = match std::env::var("WEBHOOK_URL").ok().filter(|s| !s.is_empty()) {
            None => Transport::Polling,
            Some(url) => {
                let url: Url = url.parse().context("WEBHOOK_URL is not a valid URL")?;
                let listen: SocketAddr = std::env::var("WEBHOOK_LISTEN")
                    .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
                    .parse()
                    .context("WEBHOOK_LISTEN is not a valid socket address")?;
                let secret = std::env::var("WEBHOOK_SECRET").ok().filter(|s| !s.is_empty());
                Transport::Webhook { listen, url, secret }
            }
        };

        let admin_chat_id = match std::env::var("ADMIN_CHAT_ID").ok().filter(|s| !s.is_empty()) {
            None => None,
            Some(s) => Some(
                s.parse::<i64>()
                    .context("ADMIN_CHAT_ID must be a numeric Telegram chat id")?,
            ),
        };

        Ok(Self {
            bot_token,
            database_url,
            transport,
            admin_chat_id,
        })
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,alert_bot=debug"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();
}

/// Register one command list per supported language plus an English default
/// for any user Telegram doesn't have an explicit translation for.
async fn register_command_menus(bot: &Bot) {
    let to_bot_commands = |lang| {
        messages::command_menu(lang)
            .into_iter()
            .map(|(n, d)| BotCommand::new(n, d))
            .collect::<Vec<_>>()
    };

    let de = to_bot_commands(botcore::Language::De);
    let en = to_bot_commands(botcore::Language::En);

    let (default, en_res, de_res) = tokio::join!(
        bot.set_my_commands(en.clone()).send(),
        bot.set_my_commands(en).language_code("en").send(),
        bot.set_my_commands(de).language_code("de").send(),
    );
    if let Err(e) = default {
        tracing::warn!(error = ?e, "set_my_commands (default) failed");
    }
    if let Err(e) = en_res {
        tracing::warn!(error = ?e, "set_my_commands (en) failed");
    }
    if let Err(e) = de_res {
        tracing::warn!(error = ?e, "set_my_commands (de) failed");
    }
}

fn spawn_signal_handler(
    shutdown: Arc<Notify>,
    dispatcher_token: ShutdownToken,
    notifier: AdminNotifier,
) {
    tokio::spawn(async move {
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = ?e, "failed to install SIGTERM handler");
                return;
            }
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = ?e, "failed to install SIGINT handler");
                return;
            }
        };
        let signal_name = tokio::select! {
            _ = term.recv() => "SIGTERM",
            _ = int.recv() => "SIGINT",
        };
        info!("{signal_name} received");
        notifier
            .notify(&format!("🛑 {signal_name} received, shutting down"))
            .await;

        // Tell the teloxide dispatcher to stop accepting updates. Without
        // this, `Dispatcher::dispatch{,_with_listener}` ignores SIGTERM
        // (teloxide's built-in ctrlc handler only catches SIGINT), so
        // systemd has to SIGKILL us after TimeoutStopSec — see the May 11
        // production log where a `systemctl restart` hung for 90s before
        // SIGKILL fired and the new process came up.
        if let Err(e) = dispatcher_token.shutdown() {
            tracing::warn!(error = ?e, "dispatcher shutdown failed (already idle?)");
        }
        // Wake the worker too. The dispatcher's awaiter in main() will
        // resolve on its own once `shutdown()` above propagates.
        shutdown.notify_waiters();
    });
}
