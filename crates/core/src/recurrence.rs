//! Recurrence patterns for repeating alerts.
//!
//! v1 stores `RecurrencePattern` as a serialized string in the `alerts.recurrence`
//! column but the parser does not yet emit any patterns and the worker treats
//! every alert as one-shot. v2 will hook the parser, the `next_after`
//! computation, and the worker re-schedule path.

use chrono::{DateTime, Duration, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    fn next_after(&self, fired_at: DateTime<Utc>, _tz: Tz) -> Option<DateTime<Utc>> {
        match self {
            // Simple-but-correct path; full impls land with v2.
            Self::Interval { seconds } => Some(fired_at + Duration::seconds(*seconds)),
            Self::Weekly { .. } | Self::Monthly { .. } | Self::Yearly { .. } => {
                // v2: walk forward in tz-local time, find the next matching slot.
                None
            }
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

    #[test]
    fn interval_roundtrip() {
        let p = RecurrencePattern::Interval { seconds: 86400 };
        assert_eq!(RecurrencePattern::deserialize(&p.serialize()).unwrap(), p);
    }

    #[test]
    fn weekly_roundtrip() {
        let p = RecurrencePattern::Weekly {
            days: vec![Weekday::Mon, Weekday::Wed, Weekday::Fri],
            time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
        };
        assert_eq!(RecurrencePattern::deserialize(&p.serialize()).unwrap(), p);
    }

    #[test]
    fn yearly_roundtrip() {
        let p = RecurrencePattern::Yearly {
            month: 12,
            day: 24,
            time: NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
        };
        assert_eq!(RecurrencePattern::deserialize(&p.serialize()).unwrap(), p);
    }
}
