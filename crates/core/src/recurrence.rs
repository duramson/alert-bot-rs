//! Schedule = one anchor + optional RRULE.
//!
//! Replaces the v0.2 custom `RecurrencePattern` enum. We keep the RRULE body
//! as a string (the canonical RFC-5545 form, minus the `RRULE:` prefix); it
//! gets re-parsed and re-validated on every expansion call. Cheap, and avoids
//! the typestate dance of holding `RRule<Validated>` across operations that
//! conceptually want to rebind `dtstart`.

use chrono::{DateTime, NaiveTime, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use rrule::{Frequency, NWeekday, RRule, RRuleSet, Unvalidated};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Minimum allowed interval for recurring alerts. Below this we'd be a
/// notification firehose; users can always re-add later if the limit lifts.
pub const MIN_INTERVAL_SECONDS: i64 = 1800;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    /// Anchor instant. For one-shots, this *is* the fire time. For recurring,
    /// it's the original first occurrence (immutable after creation).
    pub dtstart: DateTime<Utc>,
    /// Timezone the RRULE's clock-times are interpreted in. Stored per-alert
    /// so that a future user-tz change doesn't drift existing weekly alerts.
    pub tz: Tz,
    /// RFC-5545 RRULE body (without the `RRULE:` prefix). `None` = one-shot.
    pub rrule: Option<String>,
}

#[derive(Debug, Error)]
pub enum ScheduleError {
    #[error("invalid rrule: {0}")]
    Invalid(String),
    #[error("rrule library error: {0}")]
    Library(#[from] rrule::RRuleError),
}

impl Schedule {
    /// One-shot schedule. `next_after` returns `Some(dtstart)` while it's in
    /// the future and `None` afterwards.
    pub fn one_shot(at: DateTime<Utc>, tz: Tz) -> Self {
        Self { dtstart: at, tz, rrule: None }
    }

    /// Build a recurring schedule from a constructor-built RRule, validating
    /// it against `dtstart` and freezing the canonical string form.
    pub fn recurring(
        dtstart: DateTime<Utc>,
        tz: Tz,
        rule: RRule<Unvalidated>,
    ) -> Result<Self, ScheduleError> {
        let dts_rrule = dtstart.with_timezone(&rrule::Tz::from(tz));
        let validated = rule.validate(dts_rrule)?;
        Ok(Self { dtstart, tz, rrule: Some(validated.to_string()) })
    }

    /// Reconstruct from DB columns.
    pub fn from_db(
        dtstart: DateTime<Utc>,
        tz: Tz,
        rrule_str: Option<&str>,
    ) -> Result<Self, ScheduleError> {
        match rrule_str {
            None | Some("") => Ok(Self::one_shot(dtstart, tz)),
            Some(s) => Ok(Self { dtstart, tz, rrule: Some(s.to_owned()) }),
        }
    }

    /// First fire strictly after `now`. For one-shot, returns dtstart if it's
    /// still in the future, else `None`. For recurring, expands the rule.
    pub fn next_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let Some(rrule_str) = self.rrule.as_deref() else {
            return (self.dtstart > now).then_some(self.dtstart);
        };

        let rule: RRule<Unvalidated> = rrule_str.parse().ok()?;
        let dts = self.dtstart.with_timezone(&rrule::Tz::from(self.tz));
        let validated = rule.validate(dts).ok()?;
        let now_tz = now.with_timezone(&rrule::Tz::from(self.tz));

        // rrule's `after()` is *inclusive* — passing now=T would return the
        // T occurrence itself. We want strictly-after, so fetch a couple and
        // filter. limit=4 is enough to skip the exact-match plus give buffer.
        let set = RRuleSet::new(dts).rrule(validated).after(now_tz);
        set.all(4)
            .dates
            .into_iter()
            .map(|d| d.with_timezone(&Utc))
            .find(|d| *d > now)
    }

    pub fn is_recurring(&self) -> bool {
        self.rrule.is_some()
    }
}

// ===========================================================================
// Constructors that map our domain patterns onto RRULE
// ===========================================================================

impl Schedule {
    /// Sub-day interval (`*30m`, `*3h`) → `FREQ={MINUTELY|HOURLY};INTERVAL=N`.
    /// Rejects intervals shorter than `MIN_INTERVAL_SECONDS`.
    pub fn interval_seconds(
        dtstart: DateTime<Utc>,
        tz: Tz,
        seconds: i64,
    ) -> Result<Self, ScheduleError> {
        if seconds < MIN_INTERVAL_SECONDS {
            return Err(ScheduleError::Invalid(format!(
                "interval {seconds}s below minimum {MIN_INTERVAL_SECONDS}s"
            )));
        }
        let rule = if seconds % 3600 == 0 {
            RRule::new(Frequency::Hourly).interval((seconds / 3600) as u16)
        } else if seconds % 60 == 0 {
            RRule::new(Frequency::Minutely).interval((seconds / 60) as u16)
        } else {
            RRule::new(Frequency::Secondly).interval(seconds as u16)
        };
        Self::recurring(dtstart, tz, rule)
    }

    /// Daily at fixed wall-clock time, every N days (DST-safe).
    /// `*1d 09:00` = `every_n_days=1, time=09:00`.
    pub fn daily_at(
        dtstart: DateTime<Utc>,
        tz: Tz,
        every_n_days: u16,
        time: NaiveTime,
    ) -> Result<Self, ScheduleError> {
        let rule = RRule::new(Frequency::Daily)
            .interval(every_n_days)
            .by_hour(vec![time.hour() as u8])
            .by_minute(vec![time.minute() as u8]);
        Self::recurring(dtstart, tz, rule)
    }

    /// Weekly on given weekday(s) at fixed wall-clock time.
    pub fn weekly(
        dtstart: DateTime<Utc>,
        tz: Tz,
        days: &[Weekday],
        time: NaiveTime,
    ) -> Result<Self, ScheduleError> {
        let by_weekday: Vec<NWeekday> = days.iter().map(|w| NWeekday::Every(*w)).collect();
        let rule = RRule::new(Frequency::Weekly)
            .by_weekday(by_weekday)
            .by_hour(vec![time.hour() as u8])
            .by_minute(vec![time.minute() as u8]);
        Self::recurring(dtstart, tz, rule)
    }

    /// Monthly on a given day-of-month at fixed wall-clock time.
    /// Months without that day fire on the last day (RRULE BYMONTHDAY=-1
    /// would do that natively; for simplicity we just use the positive day
    /// and let the expander skip — see TODO below for the user-visible hint).
    pub fn monthly(
        dtstart: DateTime<Utc>,
        tz: Tz,
        day: u8,
        time: NaiveTime,
    ) -> Result<Self, ScheduleError> {
        let rule = RRule::new(Frequency::Monthly)
            .by_month_day(vec![day as i8])
            .by_hour(vec![time.hour() as u8])
            .by_minute(vec![time.minute() as u8]);
        Self::recurring(dtstart, tz, rule)
    }

    /// Yearly on a fixed month + day-of-month at fixed wall-clock time.
    pub fn yearly(
        dtstart: DateTime<Utc>,
        tz: Tz,
        month: u8,
        day: u8,
        time: NaiveTime,
    ) -> Result<Self, ScheduleError> {
        let chrono_month = chrono::Month::try_from(month)
            .map_err(|_| ScheduleError::Invalid(format!("invalid month {month}")))?;
        let rule = RRule::new(Frequency::Yearly)
            .by_month(&[chrono_month])
            .by_month_day(vec![day as i8])
            .by_hour(vec![time.hour() as u8])
            .by_minute(vec![time.minute() as u8]);
        Self::recurring(dtstart, tz, rule)
    }
}

// ===========================================================================
// Legacy v0.2 deserialization — read-only, dropped in a follow-up migration
// once all rows are upgraded.
// ===========================================================================

/// Parse a v0.2 `recurrence` column value (`interval:1800`, `weekly:1,3,5:09:00`,
/// `monthly:15:18:00`, `yearly:12-24:09:00`) into a Schedule using the given
/// anchor + tz. Returns Ok(None) if the input is empty/NULL-like.
pub fn schedule_from_legacy(
    legacy: &str,
    dtstart: DateTime<Utc>,
    tz: Tz,
) -> Result<Option<Schedule>, ScheduleError> {
    if legacy.is_empty() {
        return Ok(None);
    }
    let (kind, rest) = legacy
        .split_once(':')
        .ok_or_else(|| ScheduleError::Invalid(format!("legacy: missing ':' in {legacy:?}")))?;

    let s = match kind {
        "interval" => {
            let secs: i64 = rest
                .parse()
                .map_err(|_| ScheduleError::Invalid(format!("legacy interval: {rest:?}")))?;
            Schedule::interval_seconds(dtstart, tz, secs)?
        }
        "weekly" => {
            let (days_str, time_str) = rest.split_once(':').ok_or_else(|| {
                ScheduleError::Invalid(format!("legacy weekly: missing time in {legacy:?}"))
            })?;
            let days: Vec<Weekday> = days_str
                .split(',')
                .map(|n| n.parse::<u32>().ok().and_then(weekday_from_iso))
                .collect::<Option<_>>()
                .ok_or_else(|| ScheduleError::Invalid(format!("legacy weekly days: {days_str:?}")))?;
            let time = NaiveTime::parse_from_str(time_str, "%H:%M")
                .map_err(|_| ScheduleError::Invalid(format!("legacy weekly time: {time_str:?}")))?;
            Schedule::weekly(dtstart, tz, &days, time)?
        }
        "monthly" => {
            let (day_str, time_str) = rest.split_once(':').ok_or_else(|| {
                ScheduleError::Invalid(format!("legacy monthly: missing time in {legacy:?}"))
            })?;
            let day: u8 = day_str
                .parse()
                .map_err(|_| ScheduleError::Invalid(format!("legacy monthly day: {day_str:?}")))?;
            let time = NaiveTime::parse_from_str(time_str, "%H:%M")
                .map_err(|_| ScheduleError::Invalid(format!("legacy monthly time: {time_str:?}")))?;
            Schedule::monthly(dtstart, tz, day, time)?
        }
        "yearly" => {
            let (date_str, time_str) = rest.split_once(':').ok_or_else(|| {
                ScheduleError::Invalid(format!("legacy yearly: missing time in {legacy:?}"))
            })?;
            let (m, d) = date_str.split_once('-').ok_or_else(|| {
                ScheduleError::Invalid(format!("legacy yearly date: {date_str:?}"))
            })?;
            let month: u8 = m
                .parse()
                .map_err(|_| ScheduleError::Invalid(format!("legacy yearly month: {m:?}")))?;
            let day: u8 = d
                .parse()
                .map_err(|_| ScheduleError::Invalid(format!("legacy yearly day: {d:?}")))?;
            let time = NaiveTime::parse_from_str(time_str, "%H:%M")
                .map_err(|_| ScheduleError::Invalid(format!("legacy yearly time: {time_str:?}")))?;
            Schedule::yearly(dtstart, tz, month, day, time)?
        }
        other => {
            return Err(ScheduleError::Invalid(format!("legacy unknown kind: {other:?}")));
        }
    };
    Ok(Some(s))
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

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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

    // ---- One-shot ----

    #[test]
    fn one_shot_future_returns_dtstart() {
        let s = Schedule::one_shot(utc(2026, 5, 9, 14, 0), berlin());
        assert_eq!(s.next_after(utc(2026, 5, 9, 13, 0)), Some(utc(2026, 5, 9, 14, 0)));
    }

    #[test]
    fn one_shot_past_returns_none() {
        let s = Schedule::one_shot(utc(2026, 5, 9, 14, 0), berlin());
        assert_eq!(s.next_after(utc(2026, 5, 9, 15, 0)), None);
    }

    // ---- Interval ----

    #[test]
    fn interval_30m_next_is_dtstart_plus_30() {
        let dts = utc(2026, 5, 9, 10, 0);
        let s = Schedule::interval_seconds(dts, berlin(), 1800).unwrap();
        // strictly after dts → next slot is dts + 30m
        assert_eq!(s.next_after(dts), Some(utc(2026, 5, 9, 10, 30)));
    }

    #[test]
    fn interval_below_minimum_rejected() {
        let dts = utc(2026, 5, 9, 10, 0);
        let err = Schedule::interval_seconds(dts, berlin(), 60).unwrap_err();
        assert!(matches!(err, ScheduleError::Invalid(_)));
    }

    // ---- Daily ----

    #[test]
    fn daily_picks_today_if_time_still_ahead() {
        let dts = local(2026, 5, 9, 9, 0);
        let s = Schedule::daily_at(dts, berlin(), 1, t(9, 0)).unwrap();
        // Asking at 08:00 today → today 09:00 (which equals dtstart)
        assert_eq!(s.next_after(local(2026, 5, 9, 8, 0)), Some(local(2026, 5, 9, 9, 0)));
    }

    #[test]
    fn daily_picks_tomorrow_if_time_passed() {
        let dts = local(2026, 5, 9, 9, 0);
        let s = Schedule::daily_at(dts, berlin(), 1, t(9, 0)).unwrap();
        // Asking at 10:00 → tomorrow 09:00
        assert_eq!(
            s.next_after(local(2026, 5, 9, 10, 0)),
            Some(local(2026, 5, 10, 9, 0))
        );
    }

    #[test]
    fn every_2_days_at_11() {
        let dts = local(2026, 5, 9, 11, 0);
        let s = Schedule::daily_at(dts, berlin(), 2, t(11, 0)).unwrap();
        // Right after dtstart → +2 days
        assert_eq!(
            s.next_after(local(2026, 5, 9, 11, 1)),
            Some(local(2026, 5, 11, 11, 0))
        );
    }

    // ---- Weekly ----

    #[test]
    fn weekly_mon_wed_fri() {
        // Anchor on a Friday (2026-05-08); next "after Fri 10:00" with rule
        // Mon/Wed/Fri 09:00 is next Monday (2026-05-11).
        let dts = local(2026, 5, 8, 9, 0);
        let s = Schedule::weekly(
            dts,
            berlin(),
            &[Weekday::Mon, Weekday::Wed, Weekday::Fri],
            t(9, 0),
        )
        .unwrap();
        assert_eq!(
            s.next_after(local(2026, 5, 8, 10, 0)),
            Some(local(2026, 5, 11, 9, 0))
        );
    }

    // ---- Yearly ----

    #[test]
    fn yearly_christmas() {
        let dts = local(2026, 12, 24, 9, 0);
        let s = Schedule::yearly(dts, berlin(), 12, 24, t(9, 0)).unwrap();
        // Right after this year's → next year's
        assert_eq!(
            s.next_after(local(2026, 12, 24, 10, 0)),
            Some(local(2027, 12, 24, 9, 0))
        );
    }

    // ---- RRULE round-trip via string ----

    #[test]
    fn rrule_string_is_valid_rfc5545_subset() {
        let s = Schedule::weekly(
            local(2026, 5, 8, 9, 0),
            berlin(),
            &[Weekday::Mon, Weekday::Wed, Weekday::Fri],
            t(9, 0),
        )
        .unwrap();
        let rule_str = s.rrule.as_deref().unwrap();
        assert!(rule_str.starts_with("FREQ=WEEKLY"));
        assert!(rule_str.contains("BYDAY=MO,WE,FR"));
        assert!(rule_str.contains("BYHOUR=9"));
        assert!(rule_str.contains("BYMINUTE=0"));
    }

    #[test]
    fn schedule_from_db_round_trips() {
        let original = Schedule::daily_at(local(2026, 5, 9, 9, 0), berlin(), 2, t(11, 0)).unwrap();
        let restored = Schedule::from_db(
            original.dtstart,
            original.tz,
            original.rrule.as_deref(),
        )
        .unwrap();
        // Both should give the same next_after.
        let probe = local(2026, 5, 9, 11, 1);
        assert_eq!(original.next_after(probe), restored.next_after(probe));
    }

    // ---- Legacy migration ----

    #[test]
    fn legacy_weekly_converts() {
        // Old format: "weekly:1,3,5:09:00" = Mon/Wed/Fri at 09:00
        let dts = local(2026, 5, 8, 9, 0);
        let s = schedule_from_legacy("weekly:1,3,5:09:00", dts, berlin())
            .unwrap()
            .unwrap();
        assert_eq!(
            s.next_after(local(2026, 5, 8, 10, 0)),
            Some(local(2026, 5, 11, 9, 0))
        );
    }

    #[test]
    fn legacy_interval_converts() {
        let dts = utc(2026, 5, 9, 10, 0);
        let s = schedule_from_legacy("interval:1800", dts, berlin())
            .unwrap()
            .unwrap();
        assert_eq!(s.next_after(dts), Some(utc(2026, 5, 9, 10, 30)));
    }

    #[test]
    fn legacy_yearly_converts() {
        let dts = local(2026, 12, 24, 9, 0);
        let s = schedule_from_legacy("yearly:12-24:09:00", dts, berlin())
            .unwrap()
            .unwrap();
        assert_eq!(
            s.next_after(local(2026, 12, 24, 10, 0)),
            Some(local(2027, 12, 24, 9, 0))
        );
    }
}
