//! Locale-aware date/time rendering for confirmation and listing messages.

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;

use botcore::Language;

/// Compact form for `/list` rows.
pub fn format_local_compact(dt: DateTime<Utc>, tz: Tz, _lang: Language) -> String {
    let local = dt.with_timezone(&tz);
    format!(
        "{:02}.{:02}. {:02}:{:02}",
        local.day(),
        local.month(),
        local.hour(),
        local.minute()
    )
}

/// Short, year-included form for confirmation lines.
/// e.g. `Fr 8.5.2026 14:03` / `Fri 8 May 2026 14:03`.
pub fn format_local_short(dt: DateTime<Utc>, tz: Tz, lang: Language) -> String {
    let local = dt.with_timezone(&tz);
    let weekday = weekday_short(local.weekday(), lang);
    match lang {
        Language::De => format!(
            "{weekday} {}.{}.{} {:02}:{:02}",
            local.day(),
            local.month(),
            local.year(),
            local.hour(),
            local.minute()
        ),
        Language::En => format!(
            "{weekday} {} {} {} {:02}:{:02}",
            local.day(),
            month_name_short(local.month(), lang),
            local.year(),
            local.hour(),
            local.minute()
        ),
    }
}

fn weekday_short(d: chrono::Weekday, lang: Language) -> &'static str {
    use chrono::Weekday::*;
    match (d, lang) {
        (Mon, Language::De) => "Mo",
        (Tue, Language::De) => "Di",
        (Wed, Language::De) => "Mi",
        (Thu, Language::De) => "Do",
        (Fri, Language::De) => "Fr",
        (Sat, Language::De) => "Sa",
        (Sun, Language::De) => "So",
        (Mon, Language::En) => "Mon",
        (Tue, Language::En) => "Tue",
        (Wed, Language::En) => "Wed",
        (Thu, Language::En) => "Thu",
        (Fri, Language::En) => "Fri",
        (Sat, Language::En) => "Sat",
        (Sun, Language::En) => "Sun",
    }
}

fn month_name_short(m: u32, lang: Language) -> &'static str {
    let table_de = [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ];
    let table_en = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let idx = (m as usize).saturating_sub(1).min(11);
    match lang {
        Language::De => table_de[idx],
        Language::En => table_en[idx],
    }
}

