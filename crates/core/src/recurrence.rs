//! Recurrence patterns for repeating alerts.
//!
//! v1 stores `RecurrencePattern` as a serialized string in the `alerts.recurrence`
//! column but the parser does not yet emit any patterns and the worker treats
//! every alert as one-shot. v2 will hook the parser, the `next_after`
//! computation, and the worker re-schedule path.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Minimum allowed interval for `Interval` recurrence. Anything shorter would
/// effectively be a notification firehose; users can always re-add later if
/// the limit lifts.
pub const MIN_INTERVAL_SECONDS: i64 = 1800;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum RecurrencePattern {
    /// every N seconds (canonical form for any sub-day interval).
    Interval { seconds: i64 },

    /// e.g. `every mo,mi,fr 09:00`.
    Weekly { days: Vec<Weekday>, time: NaiveTime },

    /// e.g. `every 1. 10:00` — fires on `day` of every month.
    Monthly { day: u8, time: NaiveTime },

    /// e.g. `every 24.12 00:00`.
    Yearly { month: u8, day: u8, time: NaiveTime },
}

#[derive(Debug, Error)]
pub enum RecurrenceError {
    #[error("invalid recurrence string: {0}")]
    Invalid(String),
}

pub trait Recurrence {
    /// Compute the next fire time strictly after `fired_at` in the user's tz.
    /// Returns `None` if the recurrence has no further occurrences (e.g. an
    /// end-date was hit — currently never, reserved for v2).
    fn next_after(&self, fired_at: DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>>;
}

impl Recurrence for RecurrencePattern {
    fn next_after(&self, fired_at: DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>> {
        match self {
            Self::Interval { seconds } => Some(fired_at + Duration::seconds(*seconds)),
            Self::Weekly { days, time } => next_weekly(days, *time, fired_at, tz),
            Self::Monthly { day, time } => next_monthly(*day, *time, fired_at, tz),
            Self::Yearly { month, day, time } => next_yearly(*month, *day, *time, fired_at, tz),
        }
    }
}

/// Find the next occurrence of any weekday in `days` at `time`, strictly
/// after `fired_at`. Walks at most 8 days forward (covers a full week + today).
fn next_weekly(
    days: &[Weekday],
    time: NaiveTime,
    fired_at: DateTime<Utc>,
    tz: Tz,
) -> Option<DateTime<Utc>> {
    if days.is_empty() {
        return None;
    }
    let local_today = fired_at.with_timezone(&tz).date_naive();
    for offset in 0..=7 {
        let date = local_today.checked_add_days(chrono::Days::new(offset))?;
        if !days.contains(&date.weekday()) {
            continue;
        }
        if let Some(candidate) = local_to_utc_lenient(date, time, tz) {
            if candidate > fired_at {
                return Some(candidate);
            }
        }
    }
    None
}

/// Next occurrence of `day` of any future month, at `time`.
/// Skips months that don't have that day (e.g. Feb 31).
fn next_monthly(
    day: u8,
    time: NaiveTime,
    fired_at: DateTime<Utc>,
    tz: Tz,
) -> Option<DateTime<Utc>> {
    let local_now = fired_at.with_timezone(&tz);
    let mut year = local_now.year();
    let mut month = local_now.month();
    // Up to 14 attempts handles "every 31st" → Feb/Apr/Jun/Sep/Nov skips.
    for _ in 0..14 {
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day as u32) {
            if let Some(candidate) = local_to_utc_lenient(date, time, tz) {
                if candidate > fired_at {
                    return Some(candidate);
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
    None
}

/// Next occurrence of `month-day` at `time` in any future year.
/// Handles Feb 29 by skipping non-leap years.
fn next_yearly(
    month: u8,
    day: u8,
    time: NaiveTime,
    fired_at: DateTime<Utc>,
    tz: Tz,
) -> Option<DateTime<Utc>> {
    let local_now = fired_at.with_timezone(&tz);
    let mut year = local_now.year();
    for _ in 0..5 {
        if let Some(date) = NaiveDate::from_ymd_opt(year, month as u32, day as u32) {
            if let Some(candidate) = local_to_utc_lenient(date, time, tz) {
                if candidate > fired_at {
                    return Some(candidate);
                }
            }
        }
        year += 1;
    }
    None
}

/// Same DST-handling policy the parser uses: pick the earlier of two
/// ambiguous instants, advance by one hour out of a DST gap.
fn local_to_utc_lenient(date: NaiveDate, time: NaiveTime, tz: Tz) -> Option<DateTime<Utc>> {
    let naive = date.and_time(time);
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        chrono::LocalResult::Ambiguous(earlier, _) => Some(earlier.with_timezone(&Utc)),
        chrono::LocalResult::None => {
            // Spring-forward gap; bump 1h and retry.
            let bumped = naive + Duration::hours(1);
            tz.from_local_datetime(&bumped)
                .single()
                .map(|dt| dt.with_timezone(&Utc))
        }
    }
}

/// Compact serialization used by the `alerts.recurrence` text column.
///
/// Format: `interval:<secs>` | `weekly:<days>:<HH:MM>` | `monthly:<day>:<HH:MM>` | `yearly:<MM>-<DD>:<HH:MM>`
/// where `<days>` is comma-separated weekday numbers (mon=1 .. sun=7, ISO).
impl RecurrencePattern {
    pub fn serialize(&self) -> String {
        match self {
            Self::Interval { seconds } => format!("interval:{seconds}"),
            Self::Weekly { days, time } => {
                let days = days
                    .iter()
                    .map(|d| d.number_from_monday().to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("weekly:{days}:{}", time.format("%H:%M"))
            }
            Self::Monthly { day, time } => {
                format!("monthly:{day}:{}", time.format("%H:%M"))
            }
            Self::Yearly { month, day, time } => {
                format!("yearly:{month:02}-{day:02}:{}", time.format("%H:%M"))
            }
        }
    }

    pub fn deserialize(s: &str) -> Result<Self, RecurrenceError> {
        // Hand-rolled to avoid pulling serde-string dependencies into core.
        let mut parts = s.splitn(2, ':');
        let kind = parts.next().ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
        let rest = parts.next().ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
        match kind {
            "interval" => {
                let seconds: i64 = rest
                    .parse()
                    .map_err(|_| RecurrenceError::Invalid(s.into()))?;
                Ok(Self::Interval { seconds })
            }
            "weekly" => {
                let mut sub = rest.splitn(2, ':');
                let days_str = sub.next().ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
                let time_str = sub.next().ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
                let days = days_str
                    .split(',')
                    .map(|d| {
                        d.parse::<u32>()
                            .ok()
                            .and_then(weekday_from_iso)
                            .ok_or_else(|| RecurrenceError::Invalid(s.into()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let time = NaiveTime::parse_from_str(time_str, "%H:%M")
                    .map_err(|_| RecurrenceError::Invalid(s.into()))?;
                Ok(Self::Weekly { days, time })
            }
            "monthly" => {
                let mut sub = rest.splitn(2, ':');
                let day: u8 = sub
                    .next()
                    .and_then(|d| d.parse().ok())
                    .ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
                let time = NaiveTime::parse_from_str(
                    sub.next().ok_or_else(|| RecurrenceError::Invalid(s.into()))?,
                    "%H:%M",
                )
                .map_err(|_| RecurrenceError::Invalid(s.into()))?;
                Ok(Self::Monthly { day, time })
            }
            "yearly" => {
                let mut sub = rest.splitn(2, ':');
                let date = sub.next().ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
                let time_str = sub.next().ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
                let mut date_parts = date.split('-');
                let month: u8 = date_parts
                    .next()
                    .and_then(|m| m.parse().ok())
                    .ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
                let day: u8 = date_parts
                    .next()
                    .and_then(|d| d.parse().ok())
                    .ok_or_else(|| RecurrenceError::Invalid(s.into()))?;
                let time = NaiveTime::parse_from_str(time_str, "%H:%M")
                    .map_err(|_| RecurrenceError::Invalid(s.into()))?;
                Ok(Self::Yearly { month, day, time })
            }
            _ => Err(RecurrenceError::Invalid(s.into())),
        }
    }
}

fn weekday_from_iso(n: u32) -> Option<Weekday> {
    match n {
        1 => Some(Weekday::Mon),
        2 => Some(Weekday::Tue),
        3 => Some(Weekday::Wed),
        4 => Some(Weekday::Thu),
        5 => Some(Weekday::Fri),
        6 => Some(Weekday::Sat),
        7 => Some(Weekday::Sun),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn berlin() -> Tz {
        "Europe/Berlin".parse().unwrap()
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0).unwrap()
    }

    fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        berlin()
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn interval_roundtrip() {
        let p = RecurrencePattern::Interval { seconds: 86400 };
        assert_eq!(RecurrencePattern::deserialize(&p.serialize()).unwrap(), p);
    }

    #[test]
    fn weekly_roundtrip() {
        let p = RecurrencePattern::Weekly {
            days: vec![Weekday::Mon, Weekday::Wed, Weekday::Fri],
            time: t(9, 0),
        };
        assert_eq!(RecurrencePattern::deserialize(&p.serialize()).unwrap(), p);
    }

    #[test]
    fn yearly_roundtrip() {
        let p = RecurrencePattern::Yearly {
            month: 12,
            day: 24,
            time: t(0, 0),
        };
        assert_eq!(RecurrencePattern::deserialize(&p.serialize()).unwrap(), p);
    }

    // ---- Weekly next_after ----

    #[test]
    fn weekly_today_time_still_ahead_picks_today() {
        // Friday 2026-05-08 08:00 Berlin → next Friday 09:00 today
        let pattern = RecurrencePattern::Weekly {
            days: vec![Weekday::Fri],
            time: t(9, 0),
        };
        let from = local(2026, 5, 8, 8, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2026, 5, 8, 9, 0)));
    }

    #[test]
    fn weekly_today_time_already_passed_picks_next_week() {
        // Friday 2026-05-08 10:00 → next Friday is 2026-05-15 09:00
        let pattern = RecurrencePattern::Weekly {
            days: vec![Weekday::Fri],
            time: t(9, 0),
        };
        let from = local(2026, 5, 8, 10, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2026, 5, 15, 9, 0)));
    }

    #[test]
    fn weekly_multi_day_picks_nearest() {
        // Friday 2026-05-08 → Mo,Mi,Fr → today already past 9:00 so next Mon 11.5.
        let pattern = RecurrencePattern::Weekly {
            days: vec![Weekday::Mon, Weekday::Wed, Weekday::Fri],
            time: t(9, 0),
        };
        let from = local(2026, 5, 8, 10, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2026, 5, 11, 9, 0)));
    }

    #[test]
    fn weekly_exact_time_skips_to_next() {
        // Strictly-after: firing exactly at the same instant should jump to the
        // next slot, not return the same time again.
        let pattern = RecurrencePattern::Weekly {
            days: vec![Weekday::Mon],
            time: t(9, 0),
        };
        let from = local(2026, 5, 4, 9, 0); // Mon 09:00 Berlin
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2026, 5, 11, 9, 0)));
    }

    // ---- Monthly next_after ----

    #[test]
    fn monthly_first_of_month_today_ahead() {
        // Mid-month, "every 1st 09:00" → next month's 1st
        let pattern = RecurrencePattern::Monthly { day: 1, time: t(9, 0) };
        let from = local(2026, 5, 8, 12, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2026, 6, 1, 9, 0)));
    }

    #[test]
    fn monthly_skips_short_months() {
        // "every 31st 09:00" starting Jan 31 → skips Feb (no 31), Mar 31 next.
        let pattern = RecurrencePattern::Monthly { day: 31, time: t(9, 0) };
        let from = local(2026, 1, 31, 10, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2026, 3, 31, 9, 0)));
    }

    // ---- Yearly next_after ----

    #[test]
    fn yearly_christmas() {
        let pattern = RecurrencePattern::Yearly { month: 12, day: 24, time: t(9, 0) };
        let from = local(2026, 5, 8, 12, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2026, 12, 24, 9, 0)));
    }

    #[test]
    fn yearly_after_anniversary_rolls_to_next_year() {
        let pattern = RecurrencePattern::Yearly { month: 4, day: 30, time: t(9, 0) };
        let from = local(2026, 5, 8, 12, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2027, 4, 30, 9, 0)));
    }

    #[test]
    fn yearly_feb29_skips_non_leap() {
        // 2026 is not a leap year; Feb 29 next valid in 2028.
        let pattern = RecurrencePattern::Yearly { month: 2, day: 29, time: t(0, 0) };
        let from = utc(2026, 5, 8, 12, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(local(2028, 2, 29, 0, 0)));
    }

    // ---- Interval ----

    #[test]
    fn interval_is_simple_addition() {
        let pattern = RecurrencePattern::Interval { seconds: 1800 };
        let from = utc(2026, 5, 8, 10, 0);
        assert_eq!(pattern.next_after(from, berlin()), Some(utc(2026, 5, 8, 10, 30)));
    }
}
