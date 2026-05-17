//! Grammar implementations.
//!
//! `parse_command` is the dispatch point — strip a recurring prefix, then hand
//! off to one of the two top-level strategies. One-shot parsing tries four
//! disambiguated strategies in order: relative → absolute date → bare clock →
//! named day. Recurring reuses the relative scanner and adds weekday /
//! day-of-month / month-day strategies.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc, Weekday};

use crate::keywords::{
    compact_unit, is_at_prefix, is_in_prefix, is_recurring_keyword, is_uhr, match_longform_unit,
    match_named_day, NamedDay, TimeUnit,
};
use crate::{
    local_to_utc, Language, ParseContext, ParseError, Parsed, Schedule, DEFAULT_TIME,
    MAX_RELATIVE_YEARS, MIN_INTERVAL_SECONDS,
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

    // `heute`/`today` is intentionally not a valid spec — point user at
    // bare-clock-time. Done before other strategies so it can't be
    // accidentally swallowed by a fuzzy match.
    if let Some(first) = tokens.first() {
        if matches!(match_named_day(first.0), Some(NamedDay::Today)) {
            return Err(ParseError::HeuteRejected);
        }
    }

    let (fire_at, consumed_until, notes) = try_one_shot_strategies(&tokens, ctx)?;

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
        notes,
    })
}

fn try_one_shot_strategies(
    tokens: &[Token<'_>],
    ctx: &ParseContext,
) -> Result<(DateTime<Utc>, usize, Vec<String>), ParseError> {
    // 1. Relative (compact, inline-longform, or two-token longform).
    if let Some((spec, end_idx, default_end)) = try_relative_match(tokens)? {
        let (override_time, end_offset) =
            consume_optional_time(tokens, end_idx).unwrap_or((None, default_end));
        let (fire_at, notes) = spec.apply(ctx.now_utc, ctx.tz, override_time, ctx.language)?;
        let cap = Duration::days(MAX_RELATIVE_YEARS as i64 * 366);
        if fire_at - ctx.now_utc > cap {
            return Err(ParseError::RelTooFar(MAX_RELATIVE_YEARS));
        }
        return Ok((fire_at, end_offset, notes));
    }

    // 2. Absolute date.
    if let Some(first) = tokens.first().copied() {
        if let Some(date) = parse_date_token(first.0, ctx) {
            let (override_time, end_offset) =
                consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));
            let time = override_time.unwrap_or(DEFAULT_TIME);
            let fire_at = local_to_utc(date, time, ctx.tz)?;
            return Ok((fire_at, end_offset, Vec::new()));
        }
    }

    // 3. Bare clock-time → next occurrence (today or tomorrow).
    if let Some((time, end_offset)) = try_bare_clock(tokens) {
        let fire_at = resolve_bare_clock(time, ctx)?;
        return Ok((fire_at, end_offset, Vec::new()));
    }

    // 4. Named day (morgen / übermorgen / weekday).
    if let Some((day, override_time, end_offset)) = try_named_day(tokens) {
        let time = override_time.unwrap_or(DEFAULT_TIME);
        let date = resolve_named_day(day, time, ctx);
        let fire_at = local_to_utc(date, time, ctx.tz)?;
        return Ok((fire_at, end_offset, Vec::new()));
    }

    Err(ParseError::NoTimeExpression)
}

// ---------------------------------------------------------------------------
// Relative: combined `RelSpec` covering both compact (`1Y2M15d8h40m20s`) and
// longform single-component (`30 minuten`, `5min`).
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
struct RelSpec {
    years: u32,
    months: u32,
    weeks: u32,
    days: u32,
    hours: u32,
    minutes: u32,
    seconds: u32,
}

impl RelSpec {
    fn has_date_part(&self) -> bool {
        self.years > 0 || self.months > 0 || self.weeks > 0 || self.days > 0
    }
    fn has_sub_day(&self) -> bool {
        self.hours > 0 || self.minutes > 0 || self.seconds > 0
    }
    fn is_zero(&self) -> bool {
        !self.has_date_part() && !self.has_sub_day()
    }
    fn set(&mut self, unit: TimeUnit, n: u32) {
        match unit {
            TimeUnit::Year => self.years = n,
            TimeUnit::Month => self.months = n,
            TimeUnit::Week => self.weeks = n,
            TimeUnit::Day => self.days = n,
            TimeUnit::Hour => self.hours = n,
            TimeUnit::Minute => self.minutes = n,
            TimeUnit::Second => self.seconds = n,
        }
    }

    /// Parse compact spec: `1Y2M15d8h40m20s`, `7h30m`, `2d12h`, `5m`, `5M`.
    ///
    /// Three outcomes:
    /// - `Ok(Some(spec))` — clean parse.
    /// - `Ok(None)` — doesn't look like compact at all (e.g. `5x`, `5`).
    /// - `Err(InvalidRelSpec)` — looked like compact (consumed at least one
    ///   valid pair) but is out-of-order or incomplete (e.g. `30m2h`, `1Y2`).
    fn parse_compact(s: &str) -> Result<Option<Self>, ParseError> {
        if s.is_empty() {
            return Ok(None);
        }
        let bytes = s.as_bytes();
        let mut spec = Self::default();
        let mut i = 0;
        let mut last_rank: Option<u8> = None;
        let mut consumed_any = false;
        while i < bytes.len() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == start {
                return if consumed_any {
                    Err(ParseError::InvalidRelSpec)
                } else {
                    Ok(None)
                };
            }
            let num: u32 = match std::str::from_utf8(&bytes[start..i])
                .ok()
                .and_then(|s| s.parse().ok())
            {
                Some(n) => n,
                None => return Ok(None),
            };
            if i >= bytes.len() {
                return if consumed_any {
                    Err(ParseError::InvalidRelSpec)
                } else {
                    Ok(None)
                };
            }
            let suffix = bytes[i] as char;
            let unit = match compact_unit(suffix) {
                Some(u) => u,
                None => {
                    return if consumed_any {
                        Err(ParseError::InvalidRelSpec)
                    } else {
                        Ok(None)
                    };
                }
            };
            i += 1;
            let rank = unit_rank(unit);
            if let Some(last) = last_rank {
                if rank <= last {
                    return Err(ParseError::InvalidRelSpec);
                }
            }
            last_rank = Some(rank);
            spec.set(unit, num);
            consumed_any = true;
        }
        if spec.is_zero() {
            return Ok(None);
        }
        Ok(Some(spec))
    }

    /// Apply this spec to a base time. With `override_time`, the spec's date
    /// part is computed and the resulting date is combined with the override
    /// (sub-day in spec is rejected). Without an override:
    ///   - pure d/w/h/m/s → raw seconds added to base (DST-naive, exact arithmetic)
    ///   - involves Y or M → calendar-aware date math, wall-clock preserved on
    ///     the resulting date, then any extra w/d/h/m/s added as raw seconds.
    fn apply(
        self,
        base_utc: DateTime<Utc>,
        tz: chrono_tz::Tz,
        override_time: Option<NaiveTime>,
        lang: Language,
    ) -> Result<(DateTime<Utc>, Vec<String>), ParseError> {
        let mut notes = Vec::new();
        if self.is_zero() {
            return Err(ParseError::NoTimeExpression);
        }
        if override_time.is_some() && self.has_sub_day() {
            return Err(ParseError::SubDayRelWithOverride);
        }

        if override_time.is_none() && self.years == 0 && self.months == 0 {
            let secs = (self.weeks as i64 * 7 + self.days as i64) * 86_400
                + self.hours as i64 * 3600
                + self.minutes as i64 * 60
                + self.seconds as i64;
            return Ok((base_utc + Duration::seconds(secs), notes));
        }

        let local_base = base_utc.with_timezone(&tz);
        let base_time = local_base.time();
        let mut target_date = local_base.date_naive();

        if self.years > 0 {
            let new_year = target_date
                .year()
                .checked_add(self.years as i32)
                .ok_or_else(|| ParseError::InvalidDate(format!("year overflow on {target_date}")))?;
            target_date = match target_date.with_year(new_year) {
                Some(d) => d,
                None => {
                    // Feb 29 in non-leap target year — fall back to Feb 28.
                    notes.push(note_leap_clamp(lang, target_date, new_year));
                    NaiveDate::from_ymd_opt(new_year, target_date.month(), 28).ok_or_else(|| {
                        ParseError::InvalidDate(format!(
                            "Y add fallback failed for {target_date} -> {new_year}"
                        ))
                    })?
                }
            };
        }
        if self.months > 0 {
            let before = target_date;
            target_date = target_date
                .checked_add_months(chrono::Months::new(self.months))
                .ok_or_else(|| {
                    ParseError::InvalidDate(format!("M add: {target_date} + {}M", self.months))
                })?;
            if before.day() > target_date.day() {
                notes.push(note_month_clamp(lang, before.day(), target_date.day()));
            }
        }
        if self.weeks > 0 {
            target_date += Duration::days(self.weeks as i64 * 7);
        }
        if self.days > 0 {
            target_date += Duration::days(self.days as i64);
        }

        let time = override_time.unwrap_or(base_time);
        let mut result = local_to_utc(target_date, time, tz)?;

        if override_time.is_none() && self.has_sub_day() {
            let secs = self.hours as i64 * 3600
                + self.minutes as i64 * 60
                + self.seconds as i64;
            result += Duration::seconds(secs);
        }
        Ok((result, notes))
    }
}

fn unit_rank(unit: TimeUnit) -> u8 {
    match unit {
        TimeUnit::Year => 0,
        TimeUnit::Month => 1,
        TimeUnit::Week => 2,
        TimeUnit::Day => 3,
        TimeUnit::Hour => 4,
        TimeUnit::Minute => 5,
        TimeUnit::Second => 6,
    }
}

/// Match a relative spec at the head of the token stream.
///
/// Returns `Ok(Some(spec, next_idx, end_offset))` on a clean match,
/// `Ok(None)` if the head doesn't look relative at all (so the caller can try
/// the next strategy), and `Err(InvalidRelSpec)` when the head clearly *meant*
/// to be a compact spec but is malformed (`30m2h`, `1Y2`).
fn try_relative_match(
    tokens: &[Token<'_>],
) -> Result<Option<(RelSpec, usize, usize)>, ParseError> {
    let mut i = 0;
    if tokens.get(i).is_some_and(|t| is_in_prefix(t.0)) {
        i += 1;
    }
    let Some(first) = tokens.get(i).copied() else {
        return Ok(None);
    };

    if let Some(spec) = parse_rel_single_token(first.0)? {
        return Ok(Some((spec, i + 1, token_end(first))));
    }

    // Two-token form: bare-number + longform unit (`30 minuten`).
    let Some((num, suffix)) = split_leading_digits(first.0) else {
        return Ok(None);
    };
    if !suffix.is_empty() {
        return Ok(None);
    }
    let Some(next) = tokens.get(i + 1).copied() else {
        return Ok(None);
    };
    let Some(unit) = match_longform_unit(next.0) else {
        return Ok(None);
    };
    let mut spec = RelSpec::default();
    let n: u32 = num.try_into().map_err(|_| ParseError::InvalidRelSpec)?;
    spec.set(unit, n);
    Ok(Some((spec, i + 2, token_end(next))))
}

fn parse_rel_single_token(s: &str) -> Result<Option<RelSpec>, ParseError> {
    if let Some(spec) = RelSpec::parse_compact(s)? {
        return Ok(Some(spec));
    }
    // Inline longform: leading digits then a longform unit alias glued on.
    let Some(split) = s.find(|c: char| !c.is_ascii_digit()) else {
        return Ok(None);
    };
    if split == 0 {
        return Ok(None);
    }
    let num: u32 = match s[..split].parse() {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let Some(unit) = match_longform_unit(&s[split..]) else {
        return Ok(None);
    };
    let mut spec = RelSpec::default();
    spec.set(unit, num);
    Ok(Some(spec))
}

/// `"5m"` → `(5, "m")`, `"42"` → `(42, "")`, `"abc"` → `None`.
fn split_leading_digits(s: &str) -> Option<(i64, &str)> {
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if split == 0 {
        return None;
    }
    let num: i64 = s[..split].parse().ok()?;
    Some((num, &s[split..]))
}

fn note_leap_clamp(lang: Language, from: NaiveDate, new_year: i32) -> String {
    match lang {
        Language::De => format!(
            "29.2. → 28.2. in {new_year} (kein Schaltjahr)",
            new_year = new_year
        ),
        Language::En => format!("Feb 29 → Feb 28 in {new_year} (not a leap year)"),
    }
    .replace("{from}", &from.to_string())
}

fn note_month_clamp(lang: Language, before_day: u32, after_day: u32) -> String {
    match lang {
        Language::De => format!(
            "Tag {before_day} gibt es im Zielmonat nicht — angepasst auf {after_day}"
        ),
        Language::En => format!(
            "day {before_day} doesn't exist in the target month — clamped to {after_day}"
        ),
    }
}

// ---------------------------------------------------------------------------
// Absolute date: DD.MM[.YY|.YYYY|.] and ISO YYYY-MM-DD.
// ---------------------------------------------------------------------------

fn parse_date_token(s: &str, ctx: &ParseContext) -> Option<NaiveDate> {
    if let Some(d) = parse_iso_date(s) {
        return Some(d);
    }
    parse_dotted_date(s, ctx)
}

fn parse_iso_date(s: &str) -> Option<NaiveDate> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    if !parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let y: i32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    if !(1000..=9999).contains(&y) {
        return None;
    }
    NaiveDate::from_ymd_opt(y, m, d)
}

fn parse_dotted_date(s: &str, ctx: &ParseContext) -> Option<NaiveDate> {
    let s = s.strip_suffix('.').unwrap_or(s);
    let parts: Vec<&str> = s.split('.').collect();
    if !(parts.len() == 2 || parts.len() == 3) {
        return None;
    }
    if !parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let day: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let now_local = ctx.now_utc.with_timezone(&ctx.tz);
    let year: i32 = if parts.len() == 3 {
        let y: i32 = parts[2].parse().ok()?;
        if y < 100 {
            2000 + y
        } else {
            y
        }
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
// Bare clock-time + clock parser shared with override-consumption.
// ---------------------------------------------------------------------------

fn try_bare_clock(tokens: &[Token<'_>]) -> Option<(NaiveTime, usize)> {
    let (time, end) = consume_optional_time(tokens, 0)?;
    time.map(|t| (t, end))
}

fn resolve_bare_clock(
    time: NaiveTime,
    ctx: &ParseContext,
) -> Result<DateTime<Utc>, ParseError> {
    let today = ctx.now_utc.with_timezone(&ctx.tz).date_naive();
    let cand = local_to_utc(today, time, ctx.tz)?;
    if cand > ctx.now_utc {
        return Ok(cand);
    }
    let tomorrow = today + Duration::days(1);
    local_to_utc(tomorrow, time, ctx.tz)
}

fn consume_optional_time(
    tokens: &[Token<'_>],
    from: usize,
) -> Option<(Option<NaiveTime>, usize)> {
    if from >= tokens.len() {
        return None;
    }
    let mut i = from;
    if tokens.get(i).is_some_and(|t| is_at_prefix(t.0)) {
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

/// Accepts `14:30`, `14`, `2pm`, `2:30pm`, `12am`, `12pm`. Case-insensitive
/// for the am/pm suffix.
fn parse_clock(s: &str) -> Option<NaiveTime> {
    let lower = s.to_lowercase();
    let (body, ampm): (&str, Option<bool>) = if let Some(rest) = lower.strip_suffix("am") {
        (rest, Some(false))
    } else if let Some(rest) = lower.strip_suffix("pm") {
        (rest, Some(true))
    } else {
        (lower.as_str(), None)
    };
    if body.is_empty() {
        return None;
    }
    let (h, m): (u32, u32) = if let Some((hh, mm)) = body.split_once(':') {
        (hh.parse().ok()?, mm.parse().ok()?)
    } else {
        if !body.chars().all(|c| c.is_ascii_digit()) || body.len() > 2 {
            return None;
        }
        (body.parse().ok()?, 0)
    };
    let h = match ampm {
        Some(false) /* AM */ => {
            if h == 12 { 0 } else if h <= 11 { h } else { return None }
        }
        Some(true) /* PM */ => {
            if h == 12 { 12 } else if h <= 11 { h + 12 } else { return None }
        }
        None => h,
    };
    if h > 23 || m > 59 {
        return None;
    }
    NaiveTime::from_hms_opt(h, m, 0)
}

// ---------------------------------------------------------------------------
// Named day
// ---------------------------------------------------------------------------

fn try_named_day(
    tokens: &[Token<'_>],
) -> Option<(NamedDay, Option<NaiveTime>, usize)> {
    let first = *tokens.first()?;
    let day = match_named_day(first.0)?;
    if matches!(day, NamedDay::Today) {
        // Handled upstream — defensive guard.
        return None;
    }
    let (time, end_offset) =
        consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));
    Some((day, time, end_offset))
}

fn resolve_named_day(day: NamedDay, time: NaiveTime, ctx: &ParseContext) -> NaiveDate {
    let today = ctx.now_utc.with_timezone(&ctx.tz).date_naive();
    match day {
        NamedDay::Today => today, // unreachable in normal flow
        NamedDay::Tomorrow => today + Duration::days(1),
        NamedDay::DayAfterTomorrow => today + Duration::days(2),
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
            chrono::LocalResult::None => continue,
        };
        if candidate_utc > ctx.now_utc {
            return candidate;
        }
    }
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

    let mut notes = Vec::new();

    let (schedule, end_offset) = if let Some((sch, end)) =
        try_recurring_relative(&tokens, ctx)?
    {
        (sch, end)
    } else if let Some((sch, end)) = try_recurring_weekday(&tokens, ctx)? {
        (sch, end)
    } else if let Some((sch, end, note)) = try_recurring_day_of_month(&tokens, ctx)? {
        if let Some(n) = note {
            notes.push(n);
        }
        (sch, end)
    } else if let Some((sch, end, note)) = try_recurring_month_day(&tokens, ctx)? {
        if let Some(n) = note {
            notes.push(n);
        }
        (sch, end)
    } else {
        return Err(ParseError::NoTimeExpression);
    };

    let text = input[end_offset..].trim().to_string();
    if text.is_empty() {
        return Err(ParseError::MissingText);
    }

    Ok(Parsed {
        schedule,
        text,
        notes,
    })
}

fn try_recurring_relative(
    tokens: &[Token<'_>],
    ctx: &ParseContext,
) -> Result<Option<(Schedule, usize)>, ParseError> {
    // Recurring REL doesn't accept the `in` prefix (`*in 5m` is unidiomatic),
    // so reject upfront if it's there.
    if tokens.first().is_some_and(|t| is_in_prefix(t.0)) {
        return Ok(None);
    }
    let Some((spec, _end_idx, end_offset)) = try_relative_match(tokens)? else {
        return Ok(None);
    };
    if spec.years > 0 || spec.months > 0 {
        return Err(ParseError::InvalidRecurrenceSpec);
    }

    let total_secs = spec.weeks as i64 * 7 * 86_400
        + spec.days as i64 * 86_400
        + spec.hours as i64 * 3600
        + spec.minutes as i64 * 60
        + spec.seconds as i64;

    if total_secs < MIN_INTERVAL_SECONDS {
        return Err(ParseError::IntervalTooShort(MIN_INTERVAL_SECONDS));
    }

    let schedule = if total_secs % 86_400 == 0 {
        let every_n_days = (total_secs / 86_400) as u16;
        let dtstart = next_dtstart_for_daily(every_n_days, DEFAULT_TIME, ctx);
        Schedule::daily_at(dtstart, ctx.tz, every_n_days, DEFAULT_TIME)?
    } else {
        let dtstart = ctx.now_utc + Duration::seconds(total_secs);
        Schedule::interval_seconds(dtstart, ctx.tz, total_secs)?
    };
    Ok(Some((schedule, end_offset)))
}

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

fn try_recurring_day_of_month(
    tokens: &[Token<'_>],
    ctx: &ParseContext,
) -> Result<Option<(Schedule, usize, Option<String>)>, ParseError> {
    let Some(first) = tokens.first().copied() else {
        return Ok(None);
    };
    let Some(day) = parse_day_of_month_token(first.0) else {
        return Ok(None);
    };

    let (maybe_time, end_offset) =
        consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));
    let time = maybe_time.unwrap_or(DEFAULT_TIME);

    let (schedule, note) = if day == 31 {
        let dtstart = next_dtstart_for_monthly_last_day(time, ctx);
        let sch = Schedule::monthly_last_day(dtstart, ctx.tz, time)?;
        (sch, Some(note_last_day_of_month(ctx.language)))
    } else {
        let dtstart = next_dtstart_for_monthly(day, time, ctx);
        let sch = Schedule::monthly(dtstart, ctx.tz, day, time)?;
        (sch, None)
    };
    Ok(Some((schedule, end_offset, note)))
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

fn try_recurring_month_day(
    tokens: &[Token<'_>],
    ctx: &ParseContext,
) -> Result<Option<(Schedule, usize, Option<String>)>, ParseError> {
    let Some(first) = tokens.first().copied() else {
        return Ok(None);
    };
    let Some((day, month)) = parse_month_day_token(first.0) else {
        return Ok(None);
    };

    let (maybe_time, end_offset) =
        consume_optional_time(tokens, 1).unwrap_or((None, token_end(first)));
    let time = maybe_time.unwrap_or(DEFAULT_TIME);

    let (schedule, note) = if day == 29 && month == 2 {
        let dtstart = next_dtstart_for_yearly_last_day_of_month(month, time, ctx);
        let sch = Schedule::yearly_last_day_of_month(dtstart, ctx.tz, month, time)?;
        (sch, Some(note_leap_day_yearly(ctx.language)))
    } else {
        let dtstart = next_dtstart_for_yearly(month, day, time, ctx);
        let sch = Schedule::yearly(dtstart, ctx.tz, month, day, time)?;
        (sch, None)
    };
    Ok(Some((schedule, end_offset, note)))
}

fn parse_month_day_token(s: &str) -> Option<(u8, u8)> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 2 {
        return None;
    }
    if parts
        .iter()
        .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let day: u8 = parts[0].parse().ok()?;
    let month: u8 = parts[1].parse().ok()?;
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return None;
    }
    // Sanity-check the combination — `40.13` and friends out.
    NaiveDate::from_ymd_opt(2024, month as u32, day as u32)?;
    Some((day, month))
}

// ---------------------------------------------------------------------------
// dtstart helpers — find the first future occurrence matching the rule.
// ---------------------------------------------------------------------------

fn next_dtstart_for_daily(
    every_n_days: u16,
    time: NaiveTime,
    ctx: &ParseContext,
) -> DateTime<Utc> {
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

fn next_dtstart_for_monthly_last_day(
    time: NaiveTime,
    ctx: &ParseContext,
) -> DateTime<Utc> {
    let local_now = ctx.now_utc.with_timezone(&ctx.tz);
    let mut year = local_now.year();
    let mut month = local_now.month();
    for _ in 0..14 {
        let last_day = last_day_of_month(year, month);
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, last_day) {
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

fn next_dtstart_for_yearly_last_day_of_month(
    month: u8,
    time: NaiveTime,
    ctx: &ParseContext,
) -> DateTime<Utc> {
    let local_now = ctx.now_utc.with_timezone(&ctx.tz);
    let mut year = local_now.year();
    for _ in 0..5 {
        let last = last_day_of_month(year, month as u32);
        if let Some(date) = NaiveDate::from_ymd_opt(year, month as u32, last) {
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

fn last_day_of_month(year: i32, month: u32) -> u32 {
    // Walk back from 31 until a valid date exists. Cheaper than working out
    // leap rules by hand.
    for d in (28..=31).rev() {
        if NaiveDate::from_ymd_opt(year, month, d).is_some() {
            return d;
        }
    }
    28
}

fn note_last_day_of_month(lang: Language) -> String {
    match lang {
        Language::De => "Hinweis: in Monaten ohne 31. feuert der Reminder am letzten Tag des Monats.".to_string(),
        Language::En => "Note: in months without a 31st, this fires on the last day of the month.".to_string(),
    }
}

fn note_leap_day_yearly(lang: Language) -> String {
    match lang {
        Language::De => "Hinweis: in Nicht-Schaltjahren feuert dieser Reminder am 28.2..".to_string(),
        Language::En => "Note: in non-leap years this fires on Feb 28.".to_string(),
    }
}
