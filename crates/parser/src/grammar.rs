//! Parsing strategies for time expressions.
//!
//! All strategies operate on whitespace-tokenized input and return both the
//! resolved fire time and the byte offset at which the reminder text starts.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc, Weekday};

use crate::keywords::{
    is_at_prefix, is_in_prefix, is_uhr, match_named_day, match_time_unit, NamedDay,
};
use crate::{local_to_utc, ParseContext, ParseError, Parsed, DEFAULT_TIME};

/// (text, byte-offset where the token starts in the original input).
type Token<'a> = (&'a str, usize);

fn tokenize(input: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (idx, ch) in input.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((&input[s..idx], s));
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(s) = start {
        out.push((&input[s..], s));
    }
    out
}

/// End byte offset of a token.
fn token_end(tok: Token<'_>) -> usize {
    tok.1 + tok.0.len()
}

pub(crate) fn parse_command(input: &str, ctx: &ParseContext) -> Result<Parsed, ParseError> {
    let tokens = tokenize(input);
    if tokens.is_empty() {
        return Err(ParseError::Empty);
    }

    let (fire_at, consumed_until, had_explicit_time) = try_relative(&tokens, ctx)
        .or_else(|| try_absolute(&tokens, ctx))
        .or_else(|| try_named_day(&tokens, ctx))
        .ok_or(ParseError::NoTimeExpression)?
        .into_dt(ctx)?;

    let text = input[consumed_until..].trim().to_string();
    if text.is_empty() {
        return Err(ParseError::MissingText);
    }
    if fire_at <= ctx.now_utc {
        return Err(ParseError::InPast);
    }

    Ok(Parsed {
        fire_at,
        text,
        had_explicit_time,
    })
}

/// Intermediate result of a parsing strategy. The actual `DateTime<Utc>` is
/// computed in `into_dt` so each strategy can stay focused on the structural
/// pattern rather than tz arithmetic.
enum Match {
    Relative {
        seconds: i64,
        end_offset: usize,
    },
    Absolute {
        date: NaiveDate,
        time: Option<NaiveTime>,
        end_offset: usize,
    },
    Named {
        day: NamedDay,
        time: Option<NaiveTime>,
        end_offset: usize,
    },
}

impl Match {
    fn into_dt(self, ctx: &ParseContext) -> Result<(DateTime<Utc>, usize, bool), ParseError> {
        match self {
            Self::Relative { seconds, end_offset } => {
                let fire = ctx.now_utc + Duration::seconds(seconds);
                Ok((fire, end_offset, true))
            }
            Self::Absolute { date, time, end_offset } => {
                let had_time = time.is_some();
                let t = time.unwrap_or(DEFAULT_TIME);
                let fire = local_to_utc(date, t, ctx.tz)?;
                Ok((fire, end_offset, had_time))
            }
            Self::Named { day, time, end_offset } => {
                let had_time = time.is_some();
                let t = time.unwrap_or(DEFAULT_TIME);
                let date = resolve_named_day(day, ctx);
                let fire = local_to_utc(date, t, ctx.tz)?;
                Ok((fire, end_offset, had_time))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Relative: `5m`, `30d`, `2 stunden`, `in 30 minutes`
// ---------------------------------------------------------------------------

fn try_relative(tokens: &[Token<'_>], _ctx: &ParseContext) -> Option<Match> {
    let mut i = 0;
    if tokens.get(i).map_or(false, |t| is_in_prefix(t.0)) {
        i += 1;
    }

    let first = tokens.get(i)?;
    let (num, suffix_inline) = split_leading_digits(first.0)?;

    let (unit, end_offset) = if !suffix_inline.is_empty() {
        // `5m` → unit is the suffix part of the same token.
        (
            match_time_unit(suffix_inline)?,
            token_end(*first),
        )
    } else {
        // `5 minuten` → unit is the next token.
        let next = tokens.get(i + 1)?;
        (match_time_unit(next.0)?, token_end(*next))
    };

    Some(Match::Relative {
        seconds: unit.as_seconds(num),
        end_offset,
    })
}

/// `"5m"` → `Some((5, "m"))`, `"42"` → `Some((42, ""))`, `"abc"` → `None`.
fn split_leading_digits(s: &str) -> Option<(i64, &str)> {
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if split == 0 {
        return None;
    }
    let num: i64 = s[..split].parse().ok()?;
    Some((num, &s[split..]))
}

// ---------------------------------------------------------------------------
// Absolute: `30.4.26`, `30.04.2026 14:30`, `10.5`
// ---------------------------------------------------------------------------

fn try_absolute(tokens: &[Token<'_>], ctx: &ParseContext) -> Option<Match> {
    let first = tokens.first()?;
    let date = parse_date_token(first.0, ctx)?;

    // Optional clock token after the date.
    let (time, end_offset) = consume_optional_time(tokens, 1).unwrap_or((None, token_end(*first)));

    Some(Match::Absolute {
        date,
        time,
        end_offset,
    })
}

fn parse_date_token(s: &str, ctx: &ParseContext) -> Option<NaiveDate> {
    let parts: Vec<&str> = s.split('.').collect();
    if !(parts.len() == 2 || parts.len() == 3) {
        return None;
    }
    if !parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let day: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;

    let now_local = ctx.now_utc.with_timezone(&ctx.tz);
    let year: i32 = if parts.len() == 3 {
        let y: i32 = parts[2].parse().ok()?;
        if y < 100 { 2000 + y } else { y }
    } else {
        // No year: pick this year, roll to next year if the date already passed.
        let current = now_local.year();
        match NaiveDate::from_ymd_opt(current, month, day) {
            Some(d) if d >= now_local.date_naive() => current,
            _ => current + 1,
        }
    };

    NaiveDate::from_ymd_opt(year, month, day)
}

// ---------------------------------------------------------------------------
// Named day: `heute`, `morgen`, `übermorgen`, weekdays
// ---------------------------------------------------------------------------

fn try_named_day(tokens: &[Token<'_>], _ctx: &ParseContext) -> Option<Match> {
    let first = tokens.first()?;
    let day = match_named_day(first.0)?;

    let (time, end_offset) = consume_optional_time(tokens, 1).unwrap_or((None, token_end(*first)));

    Some(Match::Named {
        day,
        time,
        end_offset,
    })
}

fn resolve_named_day(day: NamedDay, ctx: &ParseContext) -> NaiveDate {
    let today = ctx.now_utc.with_timezone(&ctx.tz).date_naive();
    match day {
        NamedDay::Today => today,
        NamedDay::Tomorrow => today + chrono::Duration::days(1),
        NamedDay::DayAfterTomorrow => today + chrono::Duration::days(2),
        NamedDay::Weekday(target) => next_weekday(today, target),
    }
}

/// Strictly *next* occurrence of `target` — saying "Friday" on a Friday means
/// next Friday, not today.
fn next_weekday(from: NaiveDate, target: Weekday) -> NaiveDate {
    let from_n = from.weekday().num_days_from_monday() as i64;
    let target_n = target.num_days_from_monday() as i64;
    let mut diff = target_n - from_n;
    if diff <= 0 {
        diff += 7;
    }
    from + chrono::Duration::days(diff)
}

// ---------------------------------------------------------------------------
// Shared time-of-day consumer: `9`, `9:00`, `14:30`, `9 Uhr`, `14:30 Uhr`,
// `um 9`, `at 9:00`.
// ---------------------------------------------------------------------------

fn consume_optional_time(tokens: &[Token<'_>], from: usize) -> Option<(Option<NaiveTime>, usize)> {
    if from >= tokens.len() {
        return None;
    }

    let mut i = from;
    if tokens.get(i).map_or(false, |t| is_at_prefix(t.0)) {
        i += 1;
    }

    let clock_tok = tokens.get(i)?;
    let time = parse_clock(clock_tok.0)?;
    let mut end_offset = token_end(*clock_tok);
    i += 1;

    // Optional trailing "Uhr".
    if let Some(uhr) = tokens.get(i) {
        if is_uhr(uhr.0) {
            end_offset = token_end(*uhr);
        }
    }

    Some((Some(time), end_offset))
}

fn parse_clock(s: &str) -> Option<NaiveTime> {
    if let Some((h, m)) = s.split_once(':') {
        let hour: u32 = h.parse().ok()?;
        let minute: u32 = m.parse().ok()?;
        return NaiveTime::from_hms_opt(hour, minute, 0);
    }
    // Bare hour like `9` or `14`. We only accept this if it's all digits AND
    // looks like a plausible hour — otherwise we'd consume reminder words by
    // accident.
    if !s.chars().all(|c| c.is_ascii_digit()) || s.is_empty() || s.len() > 2 {
        return None;
    }
    let hour: u32 = s.parse().ok()?;
    if hour > 23 {
        return None;
    }
    NaiveTime::from_hms_opt(hour, 0, 0)
}
