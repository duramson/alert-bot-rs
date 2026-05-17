//! Command handlers: each `Command` variant maps to one async function.

use std::sync::Arc;

use chrono::Utc;
use chrono_tz::Tz;
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, BotCommandScope, ChatKind, InlineKeyboardButton, InlineKeyboardMarkup, ParseMode,
    PublicChatKind, Recipient,
};

use botcore::{AlertScope, ChatType, Language, NewAlert};
use parser::ParseContext;
use storage::PgStore;

use crate::commands::Command;
use crate::messages as m;
use crate::render;

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

pub async fn dispatch(bot: Bot, msg: Message, cmd: Command, store: Arc<PgStore>) -> HandlerResult {
    let user_id = match msg.from.as_ref() {
        Some(u) => u.id.0 as i64,
        None => return Ok(()), // anonymous channel posts: ignore
    };
    let detected_lang = Language::from_telegram_code(
        msg.from.as_ref().and_then(|u| u.language_code.as_deref()),
    );

    let user = store.upsert_user(user_id, detected_lang).await?;

    match cmd {
        Command::Start => start(bot, msg, user.language).await?,
        Command::Help => help(bot, msg, user.language).await?,
        Command::Alert(text) => {
            create_alert(&bot, &msg, &store, &user, AlertScope::Private, &text).await?
        }
        Command::Galert(text) => {
            galert(&bot, &msg, &store, &user, &text).await?;
        }
        Command::List => list(&bot, &msg, &store, user.language, user.timezone).await?,
        Command::Cancel(id) => cancel(&bot, &msg, &store, user_id, user.language, id).await?,
        Command::Tz(zone) => set_tz(&bot, &msg, &store, user_id, user.language, &zone).await?,
        Command::Lang(lang) => set_lang(&bot, &msg, &store, user_id, &lang).await?,
    }

    Ok(())
}

async fn start(bot: Bot, msg: Message, lang: Language) -> HandlerResult {
    bot.send_message(msg.chat.id, m::welcome(lang))
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

async fn help(bot: Bot, msg: Message, lang: Language) -> HandlerResult {
    bot.send_message(msg.chat.id, m::help(lang))
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

async fn create_alert(
    bot: &Bot,
    msg: &Message,
    store: &PgStore,
    user: &botcore::User,
    scope: AlertScope,
    text: &str,
) -> HandlerResult {
    let ctx = ParseContext {
        now_utc: Utc::now(),
        tz: user.timezone,
        language: user.language,
    };
    let parsed = match parser::parse(text, &ctx) {
        Ok(p) => p,
        Err(e) => {
            let reply = match e {
                parser::ParseError::Empty | parser::ParseError::MissingText => {
                    m::parse_error_missing_text(user.language)
                }
                parser::ParseError::InPast => m::parse_error_in_past(user.language),
                _ => m::parse_error_unknown(user.language),
            };
            bot.send_message(msg.chat.id, reply)
                .parse_mode(ParseMode::Html)
                .await?;
            return Ok(());
        }
    };

    let chat_type = chat_type_from_msg(msg);
    let new = NewAlert {
        user_id: user.telegram_id,
        chat_id: msg.chat.id.0,
        chat_type,
        scope,
        text: parsed.text.clone(),
        fire_at: parsed.fire_at(),
        schedule: parsed.schedule.clone(),
    };
    let alert = store.create_alert(new).await?;

    let when = render::format_local_short(alert.fire_at, user.timezone, user.language);
    let recurrence_short = render::format_recurrence_short(&alert.schedule, user.language);
    let recurrence_str = recurrence_short.as_deref();
    let reply = match scope {
        AlertScope::Private => m::alert_confirmation(user.language, &when, alert.id, recurrence_str),
        AlertScope::Shared => {
            let handle = msg
                .from
                .as_ref()
                .and_then(|u| u.username.as_ref())
                .map(|h| format!("@{h}"))
                .unwrap_or_else(|| {
                    msg.from
                        .as_ref()
                        .map(|u| u.first_name.clone())
                        .unwrap_or_default()
                });
            m::galert_confirmation(
                user.language,
                &when,
                alert.id,
                &html_escape(&handle),
                recurrence_str,
            )
        }
    };

    bot.send_message(msg.chat.id, reply)
        .reply_markup(action_keyboard(user.language, alert.id))
        .await?;
    Ok(())
}

fn action_keyboard(lang: Language, id: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        m::button_cancel(lang),
        format!("cancel:{id}"),
    )]])
}

async fn galert(
    bot: &Bot,
    msg: &Message,
    store: &PgStore,
    user: &botcore::User,
    text: &str,
) -> HandlerResult {
    let chat_type = chat_type_from_msg(msg);
    if !chat_type.is_group() {
        bot.send_message(msg.chat.id, m::galert_dm_rejected(user.language))
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }
    create_alert(bot, msg, store, user, AlertScope::Shared, text).await
}

async fn list(
    bot: &Bot,
    msg: &Message,
    store: &PgStore,
    lang: Language,
    tz: Tz,
) -> HandlerResult {
    let alerts = store.list_active_for_chat(msg.chat.id.0).await?;
    if alerts.is_empty() {
        bot.send_message(msg.chat.id, m::list_empty(lang))
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    let mut out = String::from(m::list_header(lang));
    out.push('\n');
    for a in &alerts {
        let scope_icon = match a.scope {
            AlertScope::Private => "🔒",
            AlertScope::Shared => "👥",
        };
        let when = render::format_local_compact(a.fire_at, tz, lang);
        let text = html_escape(&a.text);
        let line = match render::format_recurrence_short(&a.schedule, lang) {
            Some(rec_short) => {
                format!("\n{scope_icon}🔁 #{} · {when} · {rec_short} · {text}", a.id)
            }
            None => format!("\n{scope_icon} #{} · {when} · {text}", a.id),
        };
        out.push_str(&line);
    }

    bot.send_message(msg.chat.id, out)
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

async fn cancel(
    bot: &Bot,
    msg: &Message,
    store: &PgStore,
    requester: i64,
    lang: Language,
    id: i64,
) -> HandlerResult {
    let Some(alert) = store.get_alert(id).await? else {
        bot.send_message(msg.chat.id, m::cancel_not_found(lang)).await?;
        return Ok(());
    };
    if alert.chat_id != msg.chat.id.0 {
        // Don't reveal alerts from other chats.
        bot.send_message(msg.chat.id, m::cancel_not_found(lang)).await?;
        return Ok(());
    }
    if !alert.can_edit(requester) {
        bot.send_message(msg.chat.id, m::cancel_not_allowed(lang)).await?;
        return Ok(());
    }
    let ok = store.cancel_alert(id, requester).await?;
    let reply = if ok {
        m::cancel_ok(lang, id)
    } else {
        m::cancel_not_found(lang).to_string()
    };
    bot.send_message(msg.chat.id, reply).await?;
    Ok(())
}

async fn set_tz(
    bot: &Bot,
    msg: &Message,
    store: &PgStore,
    user_id: i64,
    lang: Language,
    zone: &str,
) -> HandlerResult {
    let trimmed = zone.trim();
    let parsed: Result<Tz, _> = trimmed.parse();
    match parsed {
        Ok(tz) => {
            store.set_timezone(user_id, tz).await?;
            bot.send_message(msg.chat.id, m::tz_set(lang, tz.name())).await?;
        }
        Err(_) => {
            bot.send_message(msg.chat.id, m::tz_invalid(lang))
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }
    Ok(())
}

async fn set_lang(
    bot: &Bot,
    msg: &Message,
    store: &PgStore,
    user_id: i64,
    lang_code: &str,
) -> HandlerResult {
    let cur_lang = store
        .get_user(user_id)
        .await?
        .map(|u| u.language)
        .unwrap_or(Language::De);
    let Some(new) = Language::parse(lang_code.trim()) else {
        bot.send_message(msg.chat.id, m::lang_invalid(cur_lang))
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    };
    store.set_language(user_id, new).await?;
    update_chat_command_menu_if_dm(bot, msg, new).await;
    bot.send_message(msg.chat.id, m::lang_set(new)).await?;
    Ok(())
}

/// Override the per-chat command menu so the slash-picker matches the bot's
/// reply language. Only effective in DMs — group menus stay on the global
/// default because the chat is shared between users with different langs.
async fn update_chat_command_menu_if_dm(bot: &Bot, msg: &Message, lang: Language) {
    if !matches!(msg.chat.kind, ChatKind::Private(_)) {
        return;
    }
    let cmds: Vec<BotCommand> = m::command_menu(lang)
        .into_iter()
        .map(|(n, d)| BotCommand::new(n, d))
        .collect();
    let scope = BotCommandScope::Chat {
        chat_id: Recipient::Id(msg.chat.id),
    };
    if let Err(e) = bot.set_my_commands(cmds).scope(scope).await {
        tracing::warn!(error = ?e, "per-chat set_my_commands failed");
    }
}

// ---------------------------------------------------------------------------
// Callback queries (inline keyboard button clicks)
// ---------------------------------------------------------------------------

pub async fn callback_dispatch(
    bot: Bot,
    query: CallbackQuery,
    store: Arc<PgStore>,
) -> HandlerResult {
    let user_id = query.from.id.0 as i64;
    let detected_lang = Language::from_telegram_code(query.from.language_code.as_deref());
    let user = store.upsert_user(user_id, detected_lang).await?;

    let data = query.data.as_deref().unwrap_or("");
    let Some((kind, id_str)) = data.split_once(':') else {
        bot.answer_callback_query(query.id.clone()).await?;
        return Ok(());
    };
    let alert_id: i64 = match id_str.parse() {
        Ok(i) => i,
        Err(_) => {
            bot.answer_callback_query(query.id.clone()).await?;
            return Ok(());
        }
    };

    match kind {
        "cancel" => handle_cancel_callback(&bot, &store, &query, user.language, alert_id).await?,
        _ => {
            bot.answer_callback_query(query.id.clone()).await?;
        }
    }
    Ok(())
}

async fn handle_cancel_callback(
    bot: &Bot,
    store: &PgStore,
    query: &CallbackQuery,
    lang: Language,
    alert_id: i64,
) -> HandlerResult {
    let requester = query.from.id.0 as i64;
    let ok = store.cancel_alert(alert_id, requester).await?;

    if ok {
        if let Some(msg) = query.message.as_ref() {
            bot.edit_message_text(msg.chat().id, msg.id(), m::cancelled_inline(lang, alert_id))
                .await
                .ok();
        }
        bot.answer_callback_query(query.id.clone())
            .text(m::callback_cancelled(lang))
            .await?;
    } else {
        bot.answer_callback_query(query.id.clone())
            .text(m::cancel_not_allowed(lang))
            .show_alert(true)
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Catch-all for messages that weren't a command.
// In DMs we point the user at /help; in groups we silently drop them so the
// bot doesn't pester everyone in the chat.
// ---------------------------------------------------------------------------

pub async fn handle_unhandled(bot: Bot, msg: Message, store: Arc<PgStore>) -> HandlerResult {
    if !matches!(msg.chat.kind, ChatKind::Private(_)) {
        return Ok(());
    }
    if msg.text().is_none() {
        return Ok(());
    }
    let Some(from) = msg.from.as_ref() else {
        return Ok(());
    };
    let user_id = from.id.0 as i64;
    let detected_lang = Language::from_telegram_code(from.language_code.as_deref());
    let user = store.upsert_user(user_id, detected_lang).await?;

    bot.send_message(msg.chat.id, m::nudge_to_help(user.language))
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn chat_type_from_msg(msg: &Message) -> ChatType {
    match &msg.chat.kind {
        ChatKind::Private(_) => ChatType::Private,
        ChatKind::Public(public) => match public.kind {
            PublicChatKind::Group(_) => ChatType::Group,
            PublicChatKind::Supergroup(_) => ChatType::Supergroup,
            PublicChatKind::Channel(_) => ChatType::Channel,
        },
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

