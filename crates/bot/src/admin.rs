//! Best-effort heads-up to a designated admin chat.
//!
//! Wired up where humans care: bot start, bot stop, signal received,
//! catastrophic errors. Failures to deliver are swallowed — we don't want a
//! sick admin notifier to take down the bot itself.

use teloxide::prelude::*;
use teloxide::types::{ChatId, ParseMode};
use tracing::{debug, warn};

#[derive(Clone)]
pub struct AdminNotifier {
    bot: Bot,
    chat_id: Option<ChatId>,
}

impl AdminNotifier {
    pub fn new(bot: Bot, chat_id: Option<i64>) -> Self {
        Self {
            bot,
            chat_id: chat_id.map(ChatId),
        }
    }

    /// Send `text` (HTML) to the admin chat, log on failure, never propagate.
    pub async fn notify(&self, text: &str) {
        let Some(id) = self.chat_id else {
            return;
        };
        match self
            .bot
            .send_message(id, text)
            .parse_mode(ParseMode::Html)
            .disable_notification(true)
            .await
        {
            Ok(_) => debug!(target: "admin", "admin notified: {text}"),
            Err(e) => warn!(target: "admin", error = ?e, "admin notification failed"),
        }
    }
}
