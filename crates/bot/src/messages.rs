//! User-facing strings, keyed by `Language`.
//!
//! Kept as a flat module of small functions rather than a `fluent` bundle —
//! the surface area is small enough that compile-time-checked Rust strings
//! beat the indirection of an FTL file plus runtime lookups.

use botcore::Language;

/// Localized command-menu entries shown in the Telegram client's `/`-picker.
/// Returned in the order they should appear.
pub fn command_menu(lang: Language) -> Vec<(&'static str, &'static str)> {
    match lang {
        Language::De => vec![
            ("start", "begrüßt dich und richtet dein Profil ein"),
            ("help", "zeigt alle Befehle und Beispiele"),
            ("alert", "neuer Reminder, privat — nur du kannst ihn editieren"),
            ("galert", "Gruppen-Reminder — jeder im Chat kann ihn editieren"),
            ("list", "listet aktive Reminder im aktuellen Chat"),
            ("cancel", "löscht einen Reminder per ID"),
            ("tz", "setzt deine Zeitzone, z.B. Europe/Berlin"),
            ("lang", "wechselt die Sprache: de oder en"),
        ],
        Language::En => vec![
            ("start", "say hi and set up your profile"),
            ("help", "show all commands and examples"),
            ("alert", "new reminder, private — only you can edit it"),
            ("galert", "group reminder — anyone in the chat can edit it"),
            ("list", "show active reminders in the current chat"),
            ("cancel", "delete a reminder by id"),
            ("tz", "set your timezone, e.g. Europe/Berlin"),
            ("lang", "switch language: de or en"),
        ],
    }
}

pub fn welcome(lang: Language) -> &'static str {
    match lang {
        Language::De => "Hi! Ich bin dein Alert-Bot. Schick mir z.B.\n\
            <code>/alert 5m Kaffee fertig</code>\n\
            <code>/alert 30.4.26 Steuererklärung</code>\n\
            <code>/alert do 14:00 Standup</code>\n\n\
            In Gruppen: <code>/galert</code> für gemeinsame Reminder.\n\
            <code>/help</code> zeigt alle Befehle.",
        Language::En => "Hi! I'm your alert bot. Try messages like\n\
            <code>/alert 5m coffee ready</code>\n\
            <code>/alert 30.4.26 file taxes</code>\n\
            <code>/alert thu 14:00 standup</code>\n\n\
            In groups: <code>/galert</code> for shared reminders.\n\
            <code>/help</code> lists every command.",
    }
}

pub fn help(lang: Language) -> &'static str {
    match lang {
        Language::De => "<b>Befehle</b>\n\
            /alert &lt;zeit&gt; &lt;text&gt; — neuer Reminder (privat, nur du editierst)\n\
            /galert &lt;zeit&gt; &lt;text&gt; — Gruppen-Reminder (jeder editiert)\n\
            /list — aktive Reminder anzeigen\n\
            /cancel &lt;id&gt; — Reminder löschen\n\
            /tz &lt;zone&gt; — Zeitzone setzen (z.B. Europe/Berlin)\n\
            /lang de|en — Sprache umschalten\n\n\
            <b>Einmalig</b>\n\
            • <code>5m</code>, <code>2h</code>, <code>30d</code>, <code>1w</code>\n\
            • <code>30.4.26</code> oder <code>30.04.2026 14:30</code>\n\
            • <code>morgen 9 Uhr</code>, <code>do 14:00</code>, <code>übermorgen</code>\n\n\
            <b>Wiederkehrend</b> (Prefix <code>*</code> oder <code>jeden</code>/<code>alle</code>)\n\
            • <code>*30m wasser</code>, <code>alle 2h pause</code>\n\
            • <code>*1d vitamin</code>, <code>jeden tag 7 Uhr aufstehen</code>\n\
            • <code>*do 14:00 standup</code>, <code>jeden mo,mi,fr 9 yoga</code>\n\
            • <code>*1. miete</code>, <code>*24.12 heiligabend</code>",
        Language::En => "<b>Commands</b>\n\
            /alert &lt;time&gt; &lt;text&gt; — new reminder (private, only you can edit)\n\
            /galert &lt;time&gt; &lt;text&gt; — group reminder (anyone can edit)\n\
            /list — show active reminders\n\
            /cancel &lt;id&gt; — delete a reminder\n\
            /tz &lt;zone&gt; — set timezone (e.g. Europe/Berlin)\n\
            /lang de|en — switch language\n\n\
            <b>One-shot</b>\n\
            • <code>5m</code>, <code>2h</code>, <code>30d</code>, <code>1w</code>\n\
            • <code>30.4.26</code> or <code>30.04.2026 14:30</code>\n\
            • <code>tomorrow 9</code>, <code>thu 14:00</code>\n\n\
            <b>Recurring</b> (prefix <code>*</code> or <code>every</code>)\n\
            • <code>*30m water</code>, <code>every 2h break</code>\n\
            • <code>*1d vitamin</code>, <code>every day 7am wake</code>\n\
            • <code>*thu 14:00 standup</code>, <code>every mon,wed,fri 9 yoga</code>\n\
            • <code>*1. rent</code>, <code>*24.12 christmas</code>",
    }
}

pub fn alert_confirmation(
    _lang: Language,
    when_short: &str,
    id: i64,
    recurrence_short: Option<&str>,
) -> String {
    match recurrence_short {
        Some(rec) => format!("✓ #{id} 🔁 · {when_short} · {rec}"),
        None => format!("✓ #{id} · {when_short}"),
    }
}

/// `creator_handle` is already HTML-escaped by the caller.
pub fn galert_confirmation(
    _lang: Language,
    when_short: &str,
    id: i64,
    creator_handle: &str,
    recurrence_short: Option<&str>,
) -> String {
    match recurrence_short {
        Some(rec) => format!("👥 #{id} 🔁 · {when_short} · {rec} · {creator_handle}"),
        None => format!("👥 #{id} · {when_short} · {creator_handle}"),
    }
}

pub fn cancelled_inline(_lang: Language, id: i64) -> String {
    format!("✗ #{id} · cancelled")
}

pub fn button_cancel(lang: Language) -> &'static str {
    match lang {
        Language::De => "✗ Löschen",
        Language::En => "✗ Cancel",
    }
}

/// Prepended to a reminder when the worker delivered it noticeably late
/// (typically because the bot was offline). `delay_human` is something like
/// "5min", "2h", "1d 3h".
pub fn delayed_prefix(lang: Language, delay_human: &str) -> String {
    match lang {
        Language::De => format!("⚠️ Verzögert um {delay_human} (Bot war offline)\n"),
        Language::En => format!("⚠️ Delayed by {delay_human} (bot was offline)\n"),
    }
}

pub fn nudge_to_help(lang: Language) -> &'static str {
    match lang {
        Language::De => "Probier <code>/alert 5m text</code> oder <code>/help</code>.",
        Language::En => "Try <code>/alert 5m text</code> or <code>/help</code>.",
    }
}

pub fn callback_cancelled(lang: Language) -> &'static str {
    match lang {
        Language::De => "Gelöscht",
        Language::En => "Cancelled",
    }
}

pub fn parse_error_unknown(lang: Language) -> &'static str {
    match lang {
        Language::De => "Konnte die Zeit nicht erkennen. <code>/help</code> zeigt Beispiele.",
        Language::En => "Couldn't parse the time. Try <code>/help</code> for examples.",
    }
}

pub fn parse_error_in_past(lang: Language) -> &'static str {
    match lang {
        Language::De => "Der Zeitpunkt liegt in der Vergangenheit.",
        Language::En => "That time is in the past.",
    }
}

pub fn parse_error_missing_text(lang: Language) -> &'static str {
    match lang {
        Language::De => "Was soll dich denn erinnern? Beispiel: <code>/alert 5m Kaffee</code>",
        Language::En => "Reminder text missing. Example: <code>/alert 5m coffee</code>",
    }
}

pub fn list_empty(lang: Language) -> &'static str {
    match lang {
        Language::De => "Keine aktiven Reminder.",
        Language::En => "No active reminders.",
    }
}

pub fn list_header(lang: Language) -> &'static str {
    match lang {
        Language::De => "<b>Aktive Reminder</b>",
        Language::En => "<b>Active reminders</b>",
    }
}

pub fn cancel_ok(lang: Language, id: i64) -> String {
    match lang {
        Language::De => format!("✗ #{id} gelöscht."),
        Language::En => format!("✗ #{id} cancelled."),
    }
}

pub fn cancel_not_allowed(lang: Language) -> &'static str {
    match lang {
        Language::De => "Du darfst diesen Reminder nicht löschen.",
        Language::En => "You can't cancel this reminder.",
    }
}

pub fn cancel_not_found(lang: Language) -> &'static str {
    match lang {
        Language::De => "Reminder nicht gefunden.",
        Language::En => "Reminder not found.",
    }
}

pub fn galert_dm_rejected(lang: Language) -> &'static str {
    match lang {
        Language::De => "<code>/galert</code> funktioniert nur in Gruppen. Im Privatchat nimm <code>/alert</code>.",
        Language::En => "<code>/galert</code> only works in groups. In DMs use <code>/alert</code>.",
    }
}

pub fn tz_set(lang: Language, tz: &str) -> String {
    match lang {
        Language::De => format!("Zeitzone gesetzt: {tz}"),
        Language::En => format!("Timezone set: {tz}"),
    }
}

pub fn tz_invalid(lang: Language) -> &'static str {
    match lang {
        Language::De => "Unbekannte Zeitzone. Beispiel: <code>/tz Europe/Berlin</code>",
        Language::En => "Unknown timezone. Example: <code>/tz Europe/Berlin</code>",
    }
}

pub fn lang_set(lang: Language) -> &'static str {
    match lang {
        Language::De => "Sprache: Deutsch",
        Language::En => "Language: English",
    }
}

pub fn lang_invalid(lang: Language) -> &'static str {
    match lang {
        Language::De => "Verfügbare Sprachen: <code>de</code>, <code>en</code>",
        Language::En => "Available languages: <code>de</code>, <code>en</code>",
    }
}
