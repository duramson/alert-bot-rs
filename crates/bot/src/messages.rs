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
            ("alert", "Reminder — nur du kannst ihn löschen (DM und Gruppen)"),
            ("galert", "Gruppen-Reminder — jeder im Chat kann ihn löschen"),
            ("list", "listet aktive Reminder im aktuellen Chat"),
            ("cancel", "löscht einen Reminder per ID"),
            ("tz", "setzt deine Zeitzone, z.B. Europe/Berlin"),
            ("lang", "wechselt die Sprache (de oder en)"),
        ],
        Language::En => vec![
            ("start", "say hi and set up your profile"),
            ("help", "show all commands and examples"),
            ("alert", "reminder — only you can cancel it (DMs and groups)"),
            ("galert", "group reminder — anyone in the chat can cancel it"),
            ("list", "show active reminders in the current chat"),
            ("cancel", "delete a reminder by id"),
            ("tz", "set your timezone, e.g. Europe/Berlin"),
            ("lang", "switch language (de or en)"),
        ],
    }
}

pub fn welcome(lang: Language) -> &'static str {
    match lang {
        Language::De => "Hi! Ich bin dein Alert-Bot. Schick mir z.B.\n\
            /alert 5m Kaffee fertig\n\
            /alert 30.4.26 Steuererklärung\n\
            /alert do 14:00 Standup\n\n\
            In Gruppen funktionieren beide: /alert für deine eigenen Reminder, \
            /galert wenn ihn jeder verwalten können soll.\n\
            /help zeigt alle Befehle.",
        Language::En => "Hi! I'm your alert bot. Try messages like\n\
            /alert 5m coffee ready\n\
            /alert 30.4.26 file taxes\n\
            /alert thu 14:00 standup\n\n\
            Both work in groups: /alert for your own reminders, /galert when \
            anyone in the chat should be able to manage it.\n\
            /help lists every command.",
    }
}

pub fn help(lang: Language) -> &'static str {
    match lang {
        Language::De => "<b>Befehle</b>\n\
            /alert &lt;zeit&gt; &lt;text&gt; — Reminder. Nur du kannst ihn löschen. Funktioniert im Privatchat und in Gruppen.\n\
            /galert &lt;zeit&gt; &lt;text&gt; — Gruppen-Reminder, jeder im Chat kann ihn löschen. Nur in Gruppen.\n\
            /list — aktive Reminder anzeigen\n\
            /cancel &lt;id&gt; — Reminder löschen\n\
            /tz &lt;zone&gt; — Zeitzone setzen (z.B. Europe/Berlin)\n\
            /lang &lt;de oder en&gt; — Sprache umschalten\n\n\
            <b>Einmalig</b>\n\
            • 5m, 2h, 30d, 1w\n\
            • 30.4.26 oder 30.04.2026 14:30\n\
            • morgen 9 Uhr, do 14:00, übermorgen\n\n\
            <b>Wiederkehrend</b> (Prefix * oder jeden/alle)\n\
            • *30m wasser, alle 2h pause\n\
            • *1d vitamin, jeden tag 7 Uhr aufstehen\n\
            • *do 14:00 standup, jeden mo,mi,fr 9 yoga\n\
            • *1. miete, *24.12 heiligabend",
        Language::En => "<b>Commands</b>\n\
            /alert &lt;time&gt; &lt;text&gt; — reminder. Only you can cancel it. Works in DMs and groups.\n\
            /galert &lt;time&gt; &lt;text&gt; — group reminder, anyone in the chat can cancel it. Groups only.\n\
            /list — show active reminders\n\
            /cancel &lt;id&gt; — delete a reminder\n\
            /tz &lt;zone&gt; — set timezone (e.g. Europe/Berlin)\n\
            /lang &lt;de or en&gt; — switch language\n\n\
            <b>One-shot</b>\n\
            • 5m, 2h, 30d, 1w\n\
            • 30.4.26 or 30.04.2026 14:30\n\
            • tomorrow 9, thu 14:00\n\n\
            <b>Recurring</b> (prefix * or every)\n\
            • *30m water, every 2h break\n\
            • *1d vitamin, every day 7am wake\n\
            • *thu 14:00 standup, every mon,wed,fri 9 yoga\n\
            • *1. rent, *24.12 christmas",
    }
}

pub fn alert_confirmation(
    lang: Language,
    when_short: &str,
    id: i64,
    recurrence_short: Option<&str>,
) -> String {
    let verb = match lang {
        Language::De => "Gespeichert",
        Language::En => "Saved",
    };
    match recurrence_short {
        Some(rec) => format!("✓ {verb} · #{id} 🔁 · {when_short} · {rec}"),
        None => format!("✓ {verb} · #{id} · {when_short}"),
    }
}

/// `creator_handle` is already HTML-escaped by the caller.
pub fn galert_confirmation(
    lang: Language,
    when_short: &str,
    id: i64,
    creator_handle: &str,
    recurrence_short: Option<&str>,
) -> String {
    let (verb, by) = match lang {
        Language::De => ("Gespeichert", "von"),
        Language::En => ("Saved", "by"),
    };
    match recurrence_short {
        Some(rec) => format!(
            "👥 {verb} · #{id} 🔁 · {when_short} · {rec} · {by} {creator_handle}"
        ),
        None => format!("👥 {verb} · #{id} · {when_short} · {by} {creator_handle}"),
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
        Language::De => "Probier /alert 5m text oder /help.",
        Language::En => "Try /alert 5m text or /help.",
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
        Language::De => "Konnte die Zeit nicht erkennen. /help zeigt Beispiele.",
        Language::En => "Couldn't parse the time. Try /help for examples.",
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
        Language::De => "Was soll dich denn erinnern? Beispiel: /alert 5m Kaffee",
        Language::En => "Reminder text missing. Example: /alert 5m coffee",
    }
}

pub fn parse_error_heute_rejected(lang: Language) -> &'static str {
    match lang {
        Language::De => "„heute“ verstehe ich nicht — schreib direkt die Uhrzeit, z.B. /alert 22:00 Bier.",
        Language::En => "I don’t take „today“ — just use the time directly, e.g. /alert 22:00 beer.",
    }
}

pub fn parse_error_subday_override(lang: Language) -> &'static str {
    match lang {
        Language::De => "Uhrzeit-Override geht nur, wenn die relative Angabe keine Stunden/Minuten/Sekunden hat. Beispiele: /alert 2d 11:00 text, /alert 1Y 9:00 text.",
        Language::En => "Clock-time override only works when the relative spec has no sub-day components. Examples: /alert 2d 11:00 text, /alert 1Y 9:00 text.",
    }
}

pub fn parse_error_rel_too_far(lang: Language, years: i32) -> String {
    match lang {
        Language::De => format!("Das ist mehr als {years} Jahre in der Zukunft — so weit gehen einmalige Reminder nicht."),
        Language::En => format!("That's more than {years} years out — one-shot reminders are capped at {years}."),
    }
}

pub fn parse_error_invalid_rel_spec(lang: Language) -> &'static str {
    match lang {
        Language::De => "Die relative Angabe sieht falsch aus — Reihenfolge ist Y>M>w>d>h>m>s, ohne Leerzeichen. Beispiel: 1Y2M15d8h30m.",
        Language::En => "That relative spec looks off — the order is Y>M>w>d>h>m>s with no spaces. Example: 1Y2M15d8h30m.",
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
        Language::De => "/galert funktioniert nur in Gruppen. Im Privatchat nimm /alert.",
        Language::En => "/galert only works in groups. In DMs use /alert.",
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
        Language::De => "Unbekannte Zeitzone. Beispiel: /tz Europe/Berlin",
        Language::En => "Unknown timezone. Example: /tz Europe/Berlin",
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
        Language::De => "Verfügbare Sprachen: de, en",
        Language::En => "Available languages: de, en",
    }
}
