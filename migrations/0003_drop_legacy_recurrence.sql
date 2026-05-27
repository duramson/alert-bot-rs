-- Drop the legacy v0.2 `recurrence` column. All recurring alerts are now
-- stored RRULE-native in the `rrule` column.
--
-- Precondition (verified against production 2026-05-27 before merge):
--   SELECT count(*) FROM alerts
--   WHERE state IN ('pending', 'claimed') AND rrule IS NULL AND recurrence IS NOT NULL;
--   -- returned 0 (two stragglers, ids 16 & 26, were backfilled first)
--
-- Irreversible: a rollback needs a backup restore, not a down-migration.
ALTER TABLE alerts DROP COLUMN recurrence;
