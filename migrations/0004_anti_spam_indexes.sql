-- Anti-spam quota lookups touch (user_id, state) and (chat_id, state). The
-- chat path already has `alerts_chat_idx`; add the matching partial index
-- for user_id so per-user quota checks stay sub-ms even at 10k+ users.
--
-- Both indexes are partial (`WHERE state IN ('pending', 'claimed')`) — we
-- only ever count *active* alerts for quotas, so historical sent/failed/
-- cancelled rows don't bloat the index.

CREATE INDEX alerts_user_active_idx
    ON alerts (user_id)
    WHERE state IN ('pending', 'claimed');
