//! Locale-aware date/time rendering for confirmation and listing messages.

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;

use botcore::{Language, RecurrencePattern};

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

/// Compact, human-readable description of a recurrence pattern, e.g.
/// `alle 30min`, `täglich 09:00`, `Mo,Mi,Fr 09:00`, `jeden 1. 09:00`.
pub fn format_recurrence_short(pattern: &RecurrencePattern, lang: Language) -> String {
    match pattern {
        RecurrencePattern::Interval { seconds } => {
            let dur = format_duration_human(*seconds);
            match lang {
                Language::De => format!("alle {dur}"),
                Language::En => format!("every {dur}"),
            }
        }
        RecurrencePattern::Weekly { days, time } => {
            let hm = format!("{:02}:{:02}", time.hour(), time.minute());
            if days.len() == 7 {
                match lang {
                    Language::De => format!("täglich {hm}"),
                    Language::En => format!("daily {hm}"),
                }
            } else {
                let mut sorted = days.clone();
                sorted.sort_by_key(|d| d.num_days_from_monday());
                let days_str = sorted
                    .iter()
                    .map(|d| weekday_short(*d, lang))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{days_str} {hm}")
            }
        }
        RecurrencePattern::Monthly { day, time } => {
            let hm = format!("{:02}:{:02}", time.hour(), time.minute());
            match lang {
                Language::De => format!("jeden {day}. {hm}"),
                Language::En => format!("monthly on {day}. {hm}"),
            }
        }
        RecurrencePattern::Yearly { month, day, time } => {
            let hm = format!("{:02}:{:02}", time.hour(), time.minute());
            match lang {
                Language::De => format!("jährlich {day}.{month}. {hm}"),
                Language::En => format!(
                    "yearly {day} {} {hm}",
                    month_name_short(*month as u32, lang)
                ),
            }
        }
    }
}

/// Compact human-readable duration, e.g. "5min", "2h 30min", "3d 4h".
pub fn format_duration_human(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes}min");
    }
    let hours = minutes / 60;
    let leftover_min = minutes % 60;
    if hours < 24 {
        if leftover_min == 0 {
            return format!("{hours}h");
        }
        return format!("{hours}h {leftover_min}min");
    }
    let days = hours / 24;
    let leftover_h = hours % 24;
    if leftover_h == 0 {
        format!("{days}d")
    } else {
        format!("{days}d {leftover_h}h")
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

