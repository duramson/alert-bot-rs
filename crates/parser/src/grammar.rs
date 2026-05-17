//! Parsing strategies for time expressions.
//!
//! All strategies operate on whitespace-tokenized input and return both the
//! resolved fire time and the byte offset at which the reminder text starts.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc, Weekday};

use crate::keywords::{
    is_at_prefix, is_in_prefix, is_recurring_keyword, is_uhr, match_named_day, match_time_unit,
    NamedDay,
};
use crate::{
    local_to_utc, ParseContext, ParseError, Parsed, Schedule, DEFAULT_TIME, MIN_INTERVAL_SECONDS,
};

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
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }

    let (recurring, rest) = strip_recurring_prefix(trimmed);
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Err(ParseError::Empty);
    }

    if recurring {
        parse_recurring(rest, ctx)
    } else {
        parse_one_shot(rest, ctx)
    }
}

/// Strip a `*` prefix or a leading recurring keyword (`every`/`alle`/`jeden`/`jede`).
fn strip_recurring_prefix(input: &str) -> (bool, &str) {
    if let Some(rest) = input.strip_prefix('*') {
        return (true, rest);
    }
    let (first, rest_with_space) = match input.split_once(char::is_whitespace) {
        Some((f, r)) => (f, r),
        None => (input, ""),
    };
    if is_recurring_keyword(first) {
        return (true, rest_with_space);
    }
    (false, input)
}

// ===========================================================================
// One-shot
// ===========================================================================

fn parse_one_shot(input: &str, ctx: &ParseContext) -> Result<Parsed, ParseError> {
    let tokens = tokenize(input);
    if tokens.is_empty() {
        return Err(ParseError::Empty);
    }

    let (fire_at, consumed_until, had_explicit_time) = try_relative(&tokens)
        .or_else(|| try_absolute(&tokens, ctx))
        .or_else(|| try_named_day(&tokens))
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
        schedule: Schedule::one_shot(fire_at, ctx.tz),
        text,
        had_explicit_time,
    })
}

/// Intermediate result of a parsing strategy.
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
                let date = resolve_named_day(day, t, ctx);
                let fire = local_to_utc(date, t, ctx.tz)?;
                Ok((fire, end_offset, had_time))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Relative: `5m`, `30d`, `2 stunden`, `in 30 minutes`
// ---------------------------------------------------------------------------

fn try_relative(tokens: &[Token<'_>]) -> Option<Match> {
    let (seconds, end_offset, _) = parse_relative(tokens, true)?;
    Some(Match::Relative { seconds, end_offset })
}

/// Shared between one-shot and recurring.
/// Returns `(seconds, end_offset, end_token_idx_exclusive)`.
/// `allow_in_prefix=false` for recurring (`*in 5m text` would be weird).
fn parse_relative(
    tokens: &[Token<'_>],
    allow_in_prefix: bool,
) -> Option<(i64, usize, usize)> {
    let mut i = 0;
    if allow_in_prefix && tokens.get(i).map_or(false, |t| is_in_prefix(t.0)) {
        i += 1;
    }

    let first = *tokens.get(i)?;
    let (num, suffix_inline) = split_leading_digits(first.0)?;

    let (unit, end_offset, end_idx) = if !suffix_inline.is_empty() {
        (
            match_time_unit(suffix_inline)?,
            token_end(first),
            i + 1,
        )
    } else {
        let next = *tokens.get(i + 1)?;
        (match_time_unit(next.0)?, token_end(next), i + 2)
    };

    Some((unit.as_seconds(num), end_offset, end_idx))
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
    let first = *tokens.first()?;
    let date = parse_date_token(first.0, ctx)?;

    let (time, end_offset) = consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));

    Some(Match::Absolute { date, time, end_offset })
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
        let current = now_local.year();
        match NaiveDate::from_ymd_opt(current, month, day) {
            Some(d) if d >= now_local.date_naive() => current,
            _ => current + 1,
        }
    };

    NaiveDate::from_ymd_opt(year, month, day)
}

// ---------------------------------------------------------------------------
// Named day (one-shot): `heute`, `morgen`, `übermorgen`, weekdays
// ---------------------------------------------------------------------------

fn try_named_day(tokens: &[Token<'_>]) -> Option<Match> {
    let first = *tokens.first()?;
    let day = match_named_day(first.0)?;

    let (time, end_offset) = consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));

    Some(Match::Named { day, time, end_offset })
}

fn resolve_named_day(day: NamedDay, time: NaiveTime, ctx: &ParseContext) -> NaiveDate {
    let today = ctx.now_utc.with_timezone(&ctx.tz).date_naive();
    match day {
        NamedDay::Today => today,
        NamedDay::Tomorrow => today + chrono::Duration::days(1),
        NamedDay::DayAfterTomorrow => today + chrono::Duration::days(2),
        // Pick the first future instance of `target` at `time` — today
        // qualifies if today's `time` hasn't passed yet, otherwise the next
        // matching weekday wins. Same simple rule for one-shot weekdays
        // as Recurrence::next_after uses for recurring ones.
        NamedDay::Weekday(target) => first_future_weekday(today, target, time, ctx),
    }
}

fn first_future_weekday(
    today: NaiveDate,
    target: Weekday,
    time: NaiveTime,
    ctx: &ParseContext,
) -> NaiveDate {
    use chrono::TimeZone;
    for offset in 0..=7 {
        let candidate = today + Duration::days(offset);
        if candidate.weekday() != target {
            continue;
        }
        let candidate_local = candidate.and_time(time);
        let candidate_utc = match ctx.tz.from_local_datetime(&candidate_local) {
            chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
            chrono::LocalResult::Ambiguous(earlier, _) => earlier.with_timezone(&Utc),
            chrono::LocalResult::None => continue, // DST gap, try next match
        };
        if candidate_utc > ctx.now_utc {
            return candidate;
        }
    }
    // Defensive: within 0..=7 we always hit the target weekday at least
    // once, so this fallback should be unreachable in practice.
    today + Duration::days(7)
}

// ===========================================================================
// Recurring
// ===========================================================================

fn parse_recurring(input: &str, ctx: &ParseContext) -> Result<Parsed, ParseError> {
    let tokens = tokenize(input);
    if tokens.is_empty() {
        return Err(ParseError::Empty);
    }

    // Try each strategy. Relative needs Result because it validates min-interval.
    let rec = if let Some(r) = try_recurring_relative(&tokens, ctx)? {
        Some(r)
    } else {
        try_recurring_weekday(&tokens, ctx)?
            .or(try_recurring_day_of_month(&tokens, ctx)?)
            .or(try_recurring_month_day(&tokens, ctx)?)
    };

    let (schedule, end_offset) = rec.ok_or(ParseError::NoTimeExpression)?;

    let text = input[end_offset..].trim().to_string();
    if text.is_empty() {
        return Err(ParseError::MissingText);
    }

    Ok(Parsed { schedule, text, had_explicit_time: false })
}

/// Recurring REL → Interval (sub-day) or Daily (`*1d` etc.).
fn try_recurring_relative(
    tokens: &[Token<'_>],
    ctx: &ParseContext,
) -> Result<Option<(Schedule, usize)>, ParseError> {
    let Some((seconds, end_offset, _)) = parse_relative(tokens, false) else {
        return Ok(None);
    };

    if seconds < MIN_INTERVAL_SECONDS {
        return Err(ParseError::IntervalTooShort(MIN_INTERVAL_SECONDS));
    }

    // Multi-of-day in seconds → Daily with default time (DST-safe).
    // Otherwise → raw Interval starting at now+interval.
    let schedule = if seconds % 86_400 == 0 && seconds > 0 {
        let every_n_days = (seconds / 86_400) as u16;
        let dtstart = next_dtstart_for_daily(every_n_days, DEFAULT_TIME, ctx);
        Schedule::daily_at(dtstart, ctx.tz, every_n_days, DEFAULT_TIME)?
    } else {
        let dtstart = ctx.now_utc + Duration::seconds(seconds);
        Schedule::interval_seconds(dtstart, ctx.tz, seconds)?
    };
    Ok(Some((schedule, end_offset)))
}

/// `*do 14:00`, `*mo,mi,fr 9`, `*montag` → weekly Schedule.
fn try_recurring_weekday(
    tokens: &[Token<'_>],
    ctx: &ParseContext,
) -> Result<Option<(Schedule, usize)>, ParseError> {
    let Some(first) = tokens.first().copied() else {
        return Ok(None);
    };
    let Some(days) = match_weekday_list(first.0) else {
        return Ok(None);
    };

    let (maybe_time, end_offset) =
        consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));
    let time = maybe_time.unwrap_or(DEFAULT_TIME);

    let dtstart = next_dtstart_for_weekly(&days, time, ctx);
    let schedule = Schedule::weekly(dtstart, ctx.tz, &days, time)?;
    Ok(Some((schedule, end_offset)))
}

fn match_weekday_list(token: &str) -> Option<Vec<Weekday>> {
    let mut out = Vec::new();
    for part in token.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        match match_named_day(part) {
            Some(NamedDay::Weekday(w)) => out.push(w),
            _ => return None,
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// `*1.` (just a day with trailing dot) → monthly Schedule.
fn try_recurring_day_of_month(
    tokens: &[Token<'_>],
    ctx: &ParseContext,
) -> Result<Option<(Schedule, usize)>, ParseError> {
    let Some(first) = tokens.first().copied() else {
        return Ok(None);
    };
    let Some(day) = parse_day_of_month_token(first.0) else {
        return Ok(None);
    };

    let (maybe_time, end_offset) =
        consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));
    let time = maybe_time.unwrap_or(DEFAULT_TIME);

    let dtstart = next_dtstart_for_monthly(day, time, ctx);
    let schedule = Schedule::monthly(dtstart, ctx.tz, day, time)?;
    Ok(Some((schedule, end_offset)))
}

fn parse_day_of_month_token(s: &str) -> Option<u8> {
    if !s.ends_with('.') {
        return None;
    }
    let digits = &s[..s.len() - 1];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let day: u8 = digits.parse().ok()?;
    if !(1..=31).contains(&day) {
        return None;
    }
    Some(day)
}

/// `*24.12` → yearly Schedule on Dec 24.
fn try_recurring_month_day(
    tokens: &[Token<'_>],
    ctx: &ParseContext,
) -> Result<Option<(Schedule, usize)>, ParseError> {
    let Some(first) = tokens.first().copied() else {
        return Ok(None);
    };
    let Some((day, month)) = parse_month_day_token(first.0) else {
        return Ok(None);
    };

    let (maybe_time, end_offset) =
        consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));
    let time = maybe_time.unwrap_or(DEFAULT_TIME);

    let dtstart = next_dtstart_for_yearly(month, day, time, ctx);
    let schedule = Schedule::yearly(dtstart, ctx.tz, month, day, time)?;
    Ok(Some((schedule, end_offset)))
}

fn parse_month_day_token(s: &str) -> Option<(u8, u8)> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 2 {
        return None;
    }
    if parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let day: u8 = parts[0].parse().ok()?;
    let month: u8 = parts[1].parse().ok()?;
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return None;
    }
    NaiveDate::from_ymd_opt(2024, month as u32, day as u32)?; // sanity check
    Some((day, month))
}

// ---------------------------------------------------------------------------
// dtstart helpers — find the first future occurrence that matches the rule,
// so the resulting Schedule's RRULE expansion starts at the right anchor.
// ---------------------------------------------------------------------------

fn next_dtstart_for_daily(every_n_days: u16, time: NaiveTime, ctx: &ParseContext) -> DateTime<Utc> {
    let today = ctx.now_utc.with_timezone(&ctx.tz).date_naive();
    let upper = every_n_days as i64 + 1;
    for offset in 0..=upper {
        let date = today + Duration::days(offset);
        if let Ok(dt) = local_to_utc(date, time, ctx.tz) {
            if dt > ctx.now_utc {
                return dt;
            }
        }
    }
    ctx.now_utc + Duration::days(every_n_days as i64)
}

fn next_dtstart_for_weekly(
    days: &[Weekday],
    time: NaiveTime,
    ctx: &ParseContext,
) -> DateTime<Utc> {
    let today = ctx.now_utc.with_timezone(&ctx.tz).date_naive();
    for offset in 0..=7 {
        let date = today + Duration::days(offset);
        if !days.contains(&date.weekday()) {
            continue;
        }
        if let Ok(dt) = local_to_utc(date, time, ctx.tz) {
            if dt > ctx.now_utc {
                return dt;
            }
        }
    }
    ctx.now_utc + Duration::days(7)
}

fn next_dtstart_for_monthly(day: u8, time: NaiveTime, ctx: &ParseContext) -> DateTime<Utc> {
    use chrono::Datelike;
    let local_now = ctx.now_utc.with_timezone(&ctx.tz);
    let mut year = local_now.year();
    let mut month = local_now.month();
    for _ in 0..14 {
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day as u32) {
            if let Ok(dt) = local_to_utc(date, time, ctx.tz) {
                if dt > ctx.now_utc {
                    return dt;
                }
            }
        }
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
    }
    ctx.now_utc + Duration::days(31)
}

fn next_dtstart_for_yearly(
    month: u8,
    day: u8,
    time: NaiveTime,
    ctx: &ParseContext,
) -> DateTime<Utc> {
    use chrono::Datelike;
    let local_now = ctx.now_utc.with_timezone(&ctx.tz);
    let mut year = local_now.year();
    for _ in 0..5 {
        if let Some(date) = NaiveDate::from_ymd_opt(year, month as u32, day as u32) {
            if let Ok(dt) = local_to_utc(date, time, ctx.tz) {
                if dt > ctx.now_utc {
                    return dt;
                }
            }
        }
        year += 1;
    }
    ctx.now_utc + Duration::days(365)
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

    let clock_tok = *tokens.get(i)?;
    let time = parse_clock(clock_tok.0)?;
    let mut end_offset = token_end(clock_tok);
    i += 1;

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
    if !s.chars().all(|c| c.is_ascii_digit()) || s.is_empty() || s.len() > 2 {
        return None;
    }
    let hour: u32 = s.parse().ok()?;
    if hour > 23 {
        return None;
    }
    NaiveTime::from_hms_opt(hour, 0, 0)
}
