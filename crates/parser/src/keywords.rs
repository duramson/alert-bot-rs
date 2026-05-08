//! Keyword tables and fuzzy lookup for the time-expression parser.
//!
//! All matching is case-insensitive. Lower-case the input before passing it
//! to any of these functions.

use chrono::Weekday;
use strsim::levenshtein;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NamedDay {
    Today,
    Tomorrow,
    DayAfterTomorrow,
    Weekday(Weekday),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TimeUnit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl TimeUnit {
    pub fn as_seconds(self, n: i64) -> i64 {
        match self {
            Self::Second => n,
            Self::Minute => n * 60,
            Self::Hour => n * 3600,
            Self::Day => n * 86_400,
            Self::Week => n * 86_400 * 7,
            // Approximate; recurring monthly/yearly uses tz-aware arithmetic in core.
            Self::Month => n * 86_400 * 30,
            Self::Year => n * 86_400 * 365,
        }
    }
}

/// Adaptive Levenshtein threshold — short keywords don't get fuzzy slack
/// because they collide with reminder words too easily.
fn max_distance_for(alias_len: usize) -> usize {
    match alias_len {
        0..=3 => 0,
        4..=5 => 1,
        _ => 2,
    }
}

/// Returns `(distance, canonical)` for the best match, or `None`.
fn best_match<T: Copy>(token: &str, table: &[(T, &[&str])]) -> Option<T> {
    let mut best: Option<(usize, T)> = None;
    for (canonical, aliases) in table {
        for alias in *aliases {
            let dist = levenshtein(token, alias);
            let max = max_distance_for(alias.len());
            if dist <= max {
                if best.map_or(true, |(d, _)| dist < d) {
                    best = Some((dist, *canonical));
                }
                if dist == 0 {
                    return Some(*canonical);
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

const NAMED_DAY_TABLE: &[(NamedDay, &[&str])] = &[
    (NamedDay::Today, &["heute", "today"]),
    (NamedDay::Tomorrow, &["morgen", "tomorrow", "tomo"]),
    (NamedDay::DayAfterTomorrow, &["übermorgen", "uebermorgen"]),
    (
        NamedDay::Weekday(Weekday::Mon),
        &["montag", "monday", "mo", "mon"],
    ),
    (
        NamedDay::Weekday(Weekday::Tue),
        &["dienstag", "tuesday", "di", "die", "tue", "tues"],
    ),
    (
        NamedDay::Weekday(Weekday::Wed),
        &["mittwoch", "wednesday", "mi", "mit", "wed"],
    ),
    (
        NamedDay::Weekday(Weekday::Thu),
        &["donnerstag", "thursday", "do", "don", "thu", "thur", "thurs"],
    ),
    (
        NamedDay::Weekday(Weekday::Fri),
        &["freitag", "friday", "fr", "fre", "fri"],
    ),
    (
        NamedDay::Weekday(Weekday::Sat),
        &["samstag", "saturday", "sa", "sam", "sat"],
    ),
    (
        NamedDay::Weekday(Weekday::Sun),
        &["sonntag", "sunday", "so", "son", "sun"],
    ),
];

const TIME_UNIT_TABLE: &[(TimeUnit, &[&str])] = &[
    (
        TimeUnit::Second,
        &["s", "sek", "sec", "secs", "sekunde", "sekunden", "second", "seconds"],
    ),
    (
        TimeUnit::Minute,
        &["m", "min", "mins", "minute", "minuten", "minutes"],
    ),
    (
        TimeUnit::Hour,
        &["h", "std", "hr", "hrs", "stunde", "stunden", "hour", "hours"],
    ),
    (
        TimeUnit::Day,
        &["d", "t", "tag", "tage", "day", "days"],
    ),
    (
        TimeUnit::Week,
        &["w", "woche", "wochen", "week", "weeks"],
    ),
    (
        TimeUnit::Month,
        &["mo", "monat", "monate", "month", "months"],
    ),
    (
        TimeUnit::Year,
        &["y", "j", "jahr", "jahre", "year", "years"],
    ),
];

const UHR_ALIASES: &[&str] = &["uhr", "o'clock", "oclock"];
const IN_ALIASES: &[&str] = &["in"];
const AT_ALIASES: &[&str] = &["um", "at"];
/// Recurring trigger words. Exact match only (case-insensitive) — fuzzy match
/// would risk swallowing reminder words like "alle" used as plural.
const RECURRING_KEYWORDS: &[&str] = &["every", "alle", "jeden", "jede"];

pub fn match_named_day(token: &str) -> Option<NamedDay> {
    let lower = token.to_lowercase();
    best_match(&lower, NAMED_DAY_TABLE)
}

pub fn match_time_unit(token: &str) -> Option<TimeUnit> {
    let lower = token.to_lowercase();
    best_match(&lower, TIME_UNIT_TABLE)
}

pub fn is_uhr(token: &str) -> bool {
    let lower = token.to_lowercase();
    UHR_ALIASES.iter().any(|a| {
        let dist = levenshtein(&lower, a);
        dist <= max_distance_for(a.len())
    })
}

pub fn is_in_prefix(token: &str) -> bool {
    let lower = token.to_lowercase();
    IN_ALIASES.iter().any(|a| lower == *a)
}

pub fn is_at_prefix(token: &str) -> bool {
    let lower = token.to_lowercase();
    AT_ALIASES.iter().any(|a| lower == *a)
}

pub fn is_recurring_keyword(token: &str) -> bool {
    let lower = token.to_lowercase();
    RECURRING_KEYWORDS.iter().any(|a| lower == *a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_named_day_de() {
        assert_eq!(match_named_day("Montag"), Some(NamedDay::Weekday(Weekday::Mon)));
        assert_eq!(match_named_day("morgen"), Some(NamedDay::Tomorrow));
    }

    #[test]
    fn exact_named_day_en() {
        assert_eq!(match_named_day("monday"), Some(NamedDay::Weekday(Weekday::Mon)));
        assert_eq!(match_named_day("tomorrow"), Some(NamedDay::Tomorrow));
    }

    #[test]
    fn fuzzy_donnerstag() {
        assert_eq!(
            match_named_day("donnerstah"),
            Some(NamedDay::Weekday(Weekday::Thu))
        );
        assert_eq!(
            match_named_day("donerstag"),
            Some(NamedDay::Weekday(Weekday::Thu))
        );
    }

    #[test]
    fn fuzzy_morgen() {
        assert_eq!(match_named_day("morgne"), Some(NamedDay::Tomorrow));
        assert_eq!(match_named_day("morgenn"), Some(NamedDay::Tomorrow));
    }

    #[test]
    fn short_aliases_are_exact_only() {
        // "do" matches Thursday exactly. "da" should NOT (would be distance 1).
        assert_eq!(
            match_named_day("do"),
            Some(NamedDay::Weekday(Weekday::Thu))
        );
        assert_eq!(match_named_day("da"), None);
    }

    #[test]
    fn unrelated_word_no_match() {
        assert_eq!(match_named_day("pizza"), None);
        assert_eq!(match_named_day("kaffee"), None);
    }

    #[test]
    fn time_unit_matches() {
        assert_eq!(match_time_unit("m"), Some(TimeUnit::Minute));
        assert_eq!(match_time_unit("min"), Some(TimeUnit::Minute));
        assert_eq!(match_time_unit("minuten"), Some(TimeUnit::Minute));
        assert_eq!(match_time_unit("h"), Some(TimeUnit::Hour));
        assert_eq!(match_time_unit("std"), Some(TimeUnit::Hour));
        assert_eq!(match_time_unit("stunden"), Some(TimeUnit::Hour));
        assert_eq!(match_time_unit("d"), Some(TimeUnit::Day));
        assert_eq!(match_time_unit("mo"), Some(TimeUnit::Month));
    }

    #[test]
    fn uhr_aliases() {
        assert!(is_uhr("Uhr"));
        assert!(is_uhr("uhr"));
        assert!(is_uhr("o'clock"));
    }
}
