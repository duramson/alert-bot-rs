//! Anti-spam limits: rate limiter + per-user/per-chat alert quotas + text length cap.
//!
//! These are *soft* guards meant to keep accidental floods and casual abuse
//! at bay. They're not a substitute for proper Telegram-side moderation (e.g.
//! removing the bot from a hostile group); they just keep the DB bounded and
//! the worker responsive when a single client sends a burst.

use std::num::NonZeroU32;

use governor::clock::DefaultClock;
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};

/// Maximum reminder-text length in characters. Telegram's own message limit
/// is 4096, but real reminders almost never need more than ~80; anything
/// past 500 is overwhelmingly junk or a DB-bloat probe.
pub const MAX_TEXT_LEN: usize = 500;

/// Maximum active (pending or claimed) alerts per Telegram user. Power users
/// can hit this with legitimate recurring schedules; that's fine — they can
/// `/cancel` something stale before adding more.
pub const MAX_ALERTS_PER_USER: i64 = 100;

/// Maximum active alerts pinned to a single chat (DM, group, or supergroup).
/// Defends against "bot added to hostile group, someone spams /galert".
pub const MAX_ALERTS_PER_CHAT: i64 = 50;

/// Commands accepted per Telegram user per minute. Bucket fills at the same
/// rate so a user can spend the whole budget at once, then trickle at one
/// command every 3s. 20/min is ~7× a power user's typical pace.
const COMMANDS_PER_MINUTE: u32 = 20;

pub type UserId = i64;

/// In-memory keyed token-bucket limiter. Lives in an `Arc` shared between
/// the message dispatcher and the callback dispatcher.
///
/// Restart resets the limiter (it's in-memory by design). That's acceptable
/// — abusers usually move on once the first attempt is throttled, and the
/// soft DB quotas catch persistent ones across restarts.
pub struct CommandRateLimiter {
    limiter: RateLimiter<UserId, DashMapStateStore<UserId>, DefaultClock>,
}

impl CommandRateLimiter {
    pub fn new() -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(COMMANDS_PER_MINUTE).unwrap());
        Self {
            limiter: RateLimiter::dashmap(quota),
        }
    }

    /// Returns `true` if the user may issue this command, `false` if they've
    /// blown through their budget for the current window.
    pub fn check(&self, user_id: UserId) -> bool {
        self.limiter.check_key(&user_id).is_ok()
    }
}

impl Default for CommandRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_allows_initial_burst() {
        let l = CommandRateLimiter::new();
        // 20/min quota → up to 20 should pass immediately.
        for _ in 0..20 {
            assert!(l.check(42), "burst within quota");
        }
    }

    #[test]
    fn rate_limiter_blocks_after_burst() {
        let l = CommandRateLimiter::new();
        for _ in 0..COMMANDS_PER_MINUTE {
            assert!(l.check(7));
        }
        // 21st in the same instant must fail.
        assert!(!l.check(7), "post-burst request blocked");
    }

    #[test]
    fn rate_limiter_is_per_user() {
        let l = CommandRateLimiter::new();
        for _ in 0..COMMANDS_PER_MINUTE {
            assert!(l.check(1));
        }
        assert!(!l.check(1), "user 1 exhausted");
        assert!(l.check(2), "user 2 still has full budget");
    }
}
