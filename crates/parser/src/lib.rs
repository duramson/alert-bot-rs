//! Time-expression parser for the alert bot.
//!
//! Parses German and English shorthand into a concrete `DateTime<Utc>` plus
//! the leftover reminder text. Designed for the `/alert` command shape:
//!
//! ```text
//! /alert 5m Kaffee fertig         → in 5 minutes
//! /alert 30d abo kündigen         → in 30 days
//! /alert 30.4.26 scheidung        → on 30 April 2026, default time
//! /alert morgen 9 Uhr Arzt        → tomorrow at 09:00
//! /alert do 14:00 Standup         → next Thursday at 14:00
//! /alert in 2 stunden Pizza       → relative with explicit "in"
//! ```
//!
//! Fuzzy matching: keywords (weekdays, "morgen", "Uhr" etc.) accept Levenshtein
//! distance ≤ 2 against any of their aliases, so common typos don't break the
//! parse. The reminder *text* is never fuzzy-matched — only the time prefix.

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use thiserror::Error;

mod keywords;

pub use botcore::Language;

/// Default time-of-day for named-day expressions without an explicit clock,
/// e.g. `morgen Arzt` → tomorrow at 09:00 local time.
pub const DEFAULT_TIME: NaiveTime = match NaiveTime::from_hms_opt(9, 0, 0) {
    Some(t) => t,
    None => unreachable!(),
};

#[derive(Debug, Clone)]
pub struct ParseContext {
    pub now_utc: DateTime<Utc>,
    pub tz: Tz,
    pub language: Language,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Parsed {
    /// Resolved fire time in UTC.
    pub fire_at: DateTime<Utc>,
    /// Reminder text — everything after the time expression, trimmed.
    pub text: String,
    /// `true` if the user gave an explicit clock time (e.g. `14:00`),
    /// `false` if we filled in `DEFAULT_TIME`. Used by rendering to decide
    /// whether to show the time component prominently.
    pub had_explicit_time: bool,
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum ParseError {
    #[error("empty input")]
    Empty,
    #[error("could not recognise a time expression at the start of the input")]
    NoTimeExpression,
    #[error("time expression is in the past")]
    InPast,
    #[error("missing reminder text")]
    MissingText,
    #[error("invalid date: {0}")]
    InvalidDate(String),
    #[error("invalid time: {0}")]
    InvalidTime(String),
}

mod grammar;

/// Top-level entry point. Tries each strategy in order:
/// relative (`5m`, `in 2 hours`), absolute date (`30.4.26`), named day (`morgen`).
pub fn parse(input: &str, ctx: &ParseContext) -> Result<Parsed, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }
    grammar::parse_command(trimmed, ctx)
}

/// Convenience for resolving a (date, time) pair in the user's tz to UTC.
/// Falls back to the next valid local time if the naive datetime falls into
/// a DST gap, and picks the earlier of two ambiguous instants.
pub(crate) fn local_to_utc(
    date: NaiveDate,
    time: NaiveTime,
    tz: Tz,
) -> Result<DateTime<Utc>, ParseError> {
    let naive = date.and_time(time);
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Ok(dt.with_timezone(&Utc)),
        chrono::LocalResult::Ambiguous(earlier, _) => Ok(earlier.with_timezone(&Utc)),
        chrono::LocalResult::None => {
            // DST gap: bump forward by an hour and retry.
            let bumped = naive
                .with_hour(naive.hour().saturating_add(1))
                .ok_or_else(|| ParseError::InvalidTime(format!("{naive}")))?;
            tz.from_local_datetime(&bumped)
                .single()
                .map(|dt| dt.with_timezone(&Utc))
                .ok_or_else(|| ParseError::InvalidTime(format!("{naive}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ctx(lang: Language) -> ParseContext {
        // Fixed reference: 2026-05-08 12:00:00 Europe/Berlin (= 10:00 UTC).
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let now_local = tz.with_ymd_and_hms(2026, 5, 8, 12, 0, 0).unwrap();
        ParseContext {
            now_utc: now_local.with_timezone(&Utc),
            tz,
            language: lang,
        }
    }

    fn p(input: &str) -> Parsed {
        parse(input, &ctx(Language::De)).expect("parse failed")
    }

    fn p_en(input: &str) -> Parsed {
        parse(input, &ctx(Language::En)).expect("parse failed")
    }

    // ---- Relative ----

    #[test]
    fn rel_minutes_short() {
        let r = p("5m Kaffee fertig");
        assert_eq!(r.text, "Kaffee fertig");
        assert_eq!(
            r.fire_at,
            ctx(Language::De).now_utc + chrono::Duration::minutes(5)
        );
        assert!(r.had_explicit_time);
    }

    #[test]
    fn rel_minutes_long_de() {
        let r = p("30 minuten Pizza");
        assert_eq!(r.text, "Pizza");
        assert_eq!(
            r.fire_at,
            ctx(Language::De).now_utc + chrono::Duration::minutes(30)
        );
    }

    #[test]
    fn rel_with_in_prefix() {
        let r = p("in 2 stunden trinken");
        assert_eq!(r.text, "trinken");
        assert_eq!(
            r.fire_at,
            ctx(Language::De).now_utc + chrono::Duration::hours(2)
        );
    }

    #[test]
    fn rel_days() {
        let r = p("30d abo kündigen");
        assert_eq!(r.text, "abo kündigen");
        assert_eq!(
            r.fire_at,
            ctx(Language::De).now_utc + chrono::Duration::days(30)
        );
    }

    #[test]
    fn rel_weeks_en() {
        let r = p_en("2w pay rent");
        assert_eq!(r.text, "pay rent");
    }

    // ---- Absolute date ----

    #[test]
    fn abs_short_year() {
        let r = p("30.4.27 scheidung einreichen");
        assert_eq!(r.text, "scheidung einreichen");
        let expected = ctx(Language::De)
            .tz
            .with_ymd_and_hms(2027, 4, 30, 9, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(r.fire_at, expected);
        assert!(!r.had_explicit_time);
    }

    #[test]
    fn abs_full_year_with_time() {
        let r = p("30.04.2027 14:30 Termin");
        assert_eq!(r.text, "Termin");
        let expected = ctx(Language::De)
            .tz
            .with_ymd_and_hms(2027, 4, 30, 14, 30, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(r.fire_at, expected);
        assert!(r.had_explicit_time);
    }

    #[test]
    fn abs_no_year_assumes_next_occurrence() {
        // 8 May → asking for "10.5" should be in 2 days, not next year.
        let r = p("10.5 Geburtstag");
        let expected = ctx(Language::De)
            .tz
            .with_ymd_and_hms(2026, 5, 10, 9, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(r.fire_at, expected);
    }

    // ---- Named day ----

    #[test]
    fn named_morgen() {
        let r = p("morgen 9 Uhr Arzt");
        let expected = ctx(Language::De)
            .tz
            .with_ymd_and_hms(2026, 5, 9, 9, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(r.fire_at, expected);
        assert_eq!(r.text, "Arzt");
        assert!(r.had_explicit_time);
    }

    #[test]
    fn named_morgen_default_time() {
        let r = p("morgen Arzt");
        let expected = ctx(Language::De)
            .tz
            .with_ymd_and_hms(2026, 5, 9, 9, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(r.fire_at, expected);
        assert!(!r.had_explicit_time);
    }

    #[test]
    fn named_uebermorgen() {
        let r = p("übermorgen 14:00 Treffen");
        let expected = ctx(Language::De)
            .tz
            .with_ymd_and_hms(2026, 5, 10, 14, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(r.fire_at, expected);
    }

    #[test]
    fn named_weekday_short() {
        // Reference is Friday 2026-05-08; "do" → next Thursday 2026-05-14.
        let r = p("do 14:00 Standup");
        let expected = ctx(Language::De)
            .tz
            .with_ymd_and_hms(2026, 5, 14, 14, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(r.fire_at, expected);
    }

    #[test]
    fn named_weekday_long_en() {
        let r = p_en("monday 10:00 standup");
        // Reference Friday 2026-05-08 → next Monday 2026-05-11.
        let expected = ctx(Language::En)
            .tz
            .with_ymd_and_hms(2026, 5, 11, 10, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(r.fire_at, expected);
    }

    // ---- Fuzzy matching ----

    #[test]
    fn fuzzy_typo_morgen() {
        let r = p("morgne 10 Uhr Arzt");
        assert_eq!(r.text, "Arzt");
    }

    #[test]
    fn fuzzy_typo_donnerstag() {
        let r = p("donnerstah 14 Standup");
        assert_eq!(r.text, "Standup");
    }

    #[test]
    fn fuzzy_does_not_eat_unrelated_words() {
        // "Pizza" is nowhere near a time keyword, must not be consumed.
        let err = parse("Pizza essen", &ctx(Language::De)).unwrap_err();
        assert_eq!(err, ParseError::NoTimeExpression);
    }

    // ---- Errors ----

    #[test]
    fn err_no_text() {
        let err = parse("5m", &ctx(Language::De)).unwrap_err();
        assert_eq!(err, ParseError::MissingText);
    }

    #[test]
    fn err_in_past_absolute() {
        let err = parse("01.01.2020 foo", &ctx(Language::De)).unwrap_err();
        assert_eq!(err, ParseError::InPast);
    }

    #[test]
    fn err_no_time_expression() {
        let err = parse("hallo welt", &ctx(Language::De)).unwrap_err();
        assert_eq!(err, ParseError::NoTimeExpression);
    }
}
