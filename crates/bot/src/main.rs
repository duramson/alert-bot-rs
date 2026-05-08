use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use teloxide::update_listeners::webhooks;
use teloxide::prelude::*;
use teloxide::types::BotCommand;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use url::Url;

use storage::PgStore;

mod commands;
mod handlers;
mod messages;
mod render;
mod worker;

use commands::Command;

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

    let shutdown = Arc::new(Notify::new());
    spawn_signal_handler(shutdown.clone());

    let worker_handle = {
        let bot = bot.clone();
        let store = store.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move { worker::run(bot, store, shutdown).await })
    };

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .filter_command::<Command>()
                .endpoint(handlers::dispatch),
        )
        .branch(Update::filter_callback_query().endpoint(handlers::callback_dispatch))
        .branch(Update::filter_message().endpoint(handlers::handle_unhandled));

    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .dependencies(dptree::deps![store.clone()])
        .enable_ctrlc_handler()
        .build();

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
    let _ = worker_handle.await;
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

        Ok(Self {
            bot_token,
            database_url,
            transport,
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

fn spawn_signal_handler(shutdown: Arc<Notify>) {
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
        tokio::select! {
            _ = term.recv() => info!("SIGTERM received"),
            _ = int.recv() => info!("SIGINT received"),
        }
        shutdown.notify_waiters();
    });
}
