-- Users: one row per Telegram user that has interacted with the bot.
CREATE TABLE users (
    telegram_id   BIGINT PRIMARY KEY,
    timezone      TEXT NOT NULL DEFAULT 'Europe/Berlin',
    language      TEXT NOT NULL DEFAULT 'de',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Alerts: one row per scheduled reminder (one-shot in v1, recurring in v2).
--
-- scope:
--   'private' — only the creator (user_id) can edit/delete (`/alert`)
--   'shared'  — any member of chat_id can edit/delete (`/galert`)
--
-- chat_type:
--   'private' — DM with the bot (chat_id == user_id)
--   'group' / 'supergroup' / 'channel' — group conversation
--
-- state machine:
--   pending → claimed → sent
--                    └→ failed (after attempts >= max)
--   pending → cancelled (user-initiated)
CREATE TABLE alerts (
    id              BIGSERIAL PRIMARY KEY,
    user_id         BIGINT NOT NULL REFERENCES users(telegram_id),
    chat_id         BIGINT NOT NULL,
    chat_type       TEXT NOT NULL,
    scope           TEXT NOT NULL CHECK (scope IN ('private', 'shared')),
    text            TEXT NOT NULL,
    fire_at         TIMESTAMPTZ NOT NULL,
    recurrence      TEXT,
    state           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (state IN ('pending', 'claimed', 'sent', 'failed', 'cancelled')),
    attempts        SMALLINT NOT NULL DEFAULT 0,
    last_error      TEXT,
    claimed_at      TIMESTAMPTZ,
    fired_at        TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Hot path: worker query "give me the next due pending alerts".
CREATE INDEX alerts_due_idx ON alerts (fire_at) WHERE state = 'pending';

-- Used by /list and to enforce per-chat scoping.
CREATE INDEX alerts_chat_idx ON alerts (chat_id) WHERE state IN ('pending', 'claimed');

-- Idempotency: every Telegram update_id we've already processed.
-- A reaper deletes rows older than ~24h.
CREATE TABLE processed_updates (
    update_id     BIGINT PRIMARY KEY,
    received_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX processed_updates_received_at_idx ON processed_updates (received_at);

-- NOTIFY pipeline: insert/update on alerts wakes the worker without polling.
CREATE OR REPLACE FUNCTION notify_alert_change() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('alerts_changed', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER alerts_notify_change
AFTER INSERT OR UPDATE OF fire_at, state ON alerts
FOR EACH ROW EXECUTE FUNCTION notify_alert_change();
