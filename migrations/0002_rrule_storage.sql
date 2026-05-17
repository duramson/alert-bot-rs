-- Move recurrence storage from a custom text format to RFC-5545 RRULE.
--
-- Adds three columns on `alerts`:
--   * dtstart  — anchor instant; immutable after creation. For one-shot
--                alerts this equals fire_at at creation. For recurring,
--                it's the original first occurrence used as the RRULE anchor.
--   * rrule    — RFC-5545 RRULE body (no "RRULE:" prefix). NULL = one-shot.
--   * tz       — IANA timezone the RRULE's wall-clock times resolve in.
--                Stored per-alert so user-tz changes don't drift existing
--                weekly/monthly alerts.
--
-- The legacy `recurrence` column stays for one more cycle; storage code
-- reads it as a fallback when `rrule` is NULL but `recurrence` is set, then
-- writes it back as a proper RRULE on the next mutation. A follow-up
-- migration drops the legacy column once all live rows are upgraded.

ALTER TABLE alerts ADD COLUMN dtstart TIMESTAMPTZ;
ALTER TABLE alerts ADD COLUMN rrule   TEXT;
ALTER TABLE alerts ADD COLUMN tz      TEXT;

-- Backfill existing rows:
--   * dtstart = fire_at (best anchor — for recurring alerts the original
--     creation-time anchor is lost, but the *next* fire is still a valid
--     anchor for RRULE expansion going forward).
--   * tz      = creator's current tz (good-enough; was effectively the case
--     before this migration too since recurrence used user.timezone).
UPDATE alerts a
SET dtstart = a.fire_at,
    tz      = u.timezone
FROM users u
WHERE a.user_id = u.telegram_id;

-- For any orphan rows (user row deleted but alert row still around — shouldn't
-- happen given the FK, but be defensive), fall back to UTC.
UPDATE alerts SET tz = 'UTC' WHERE tz IS NULL;
UPDATE alerts SET dtstart = fire_at WHERE dtstart IS NULL;

ALTER TABLE alerts ALTER COLUMN dtstart SET NOT NULL;
ALTER TABLE alerts ALTER COLUMN tz      SET NOT NULL;
