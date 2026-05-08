# alert-bot-rs

A Telegram reminder bot in Rust. German and English natural language, fuzzy
matching against typos, group reminders with per-user permissions, no LLM
required — runs deterministically on your own hardware.

```
You:   /alert 5m Kaffee fertig
Bot:   ✓ #1 · Fr 8.5.2026 14:03
       [✗ Löschen]

You:   /alert do 14:00 Standup
Bot:   ✓ #2 · Do 14.5.2026 14:00
       [✗ Löschen]

You:   /alert 30.4.27 scheidung einreichen
Bot:   ✓ #3 · Fr 30.4.2027 09:00
       [✗ Löschen]
```

## Features

- **Compact slash syntax** — `/alert 5m text`, `/alert 30d text`,
  `/alert 30.4.26 text`, `/alert morgen 9 Uhr text`, `/alert do 14:00 text`.
- **Recurring** — `*` prefix or `every`/`alle`/`jeden`. `*30m water`,
  `*1d vitamin`, `*do 14:00 standup`, `*mo,mi,fr 9 yoga`, `*1. rent`,
  `*24.12 christmas`. Minimum interval 30 minutes.
- **DE + EN parser** — keywords (`heute`/`today`, `morgen`/`tomorrow`, weekdays,
  `Uhr`, time units) accept both languages and tolerate typos via Levenshtein
  distance with adaptive thresholds.
- **Group reminders with two scopes**
  - `/alert` in a group fires in the group, but only the creator can cancel it.
  - `/galert` is a shared group reminder; anyone in the chat can cancel.
- **Per-user language and timezone** — auto-detected from Telegram's
  `language_code`, overridable via `/lang` and `/tz`. The slash-menu also
  switches to the right language when `/lang` is used in a DM.
- **Webhook or polling** — long-polling for local development, webhook + TLS
  via Cloudflare Tunnel (or any reverse proxy) for production.
- **Postgres-backed delivery** — workers claim due alerts via
  `FOR UPDATE SKIP LOCKED`, listen on `NOTIFY` so they don't poll, and
  redrive stuck claims after a worker crash.
- **No LLM, no API keys beyond Telegram** — fully self-hosted.

## Quick start

```bash
git clone <your-fork>
cd alert-bot-rs
cp .env.example .env
# edit .env: BOT_TOKEN (from @BotFather) and POSTGRES_PASSWORD
docker compose up --build
```

That brings up Postgres and the bot in long-polling mode — no public endpoint
needed. Open Telegram, message your bot, send `/start`.

## Configuration

| Variable             | Required | Default              | Notes                                                    |
|----------------------|----------|----------------------|----------------------------------------------------------|
| `BOT_TOKEN`          | yes      | —                    | from [@BotFather](https://t.me/BotFather)                |
| `POSTGRES_PASSWORD`  | yes      | —                    | also set in compose                                       |
| `POSTGRES_USER`      | no       | `alertbot`           |                                                          |
| `POSTGRES_DB`        | no       | `alertbot`           |                                                          |
| `WEBHOOK_URL`        | no       | empty (long-polling) | public HTTPS URL Telegram will POST to                   |
| `WEBHOOK_LISTEN`     | no       | `0.0.0.0:8080`       | bind address inside the container                        |
| `WEBHOOK_SECRET`     | no       | empty                | recommended; sent as `X-Telegram-Bot-Api-Secret-Token`   |
| `CLOUDFLARED_TOKEN`  | no       | empty                | only needed for the `tunnel` profile                     |
| `RUST_LOG`           | no       | `info,alert_bot=debug` |                                                       |

## Deployment

### Local / Mac mini / homelab with Cloudflare Tunnel

The compose file ships an optional `cloudflared` service behind the `tunnel`
profile. It exposes the bot to the public internet through Cloudflare without
opening any ports on the host.

1. Create a tunnel in the Cloudflare Zero Trust dashboard.
2. Add a public hostname route → service `http://bot:8080`.
3. Copy the tunnel token into `.env` as `CLOUDFLARED_TOKEN`.
4. Set `WEBHOOK_URL` to the public hostname and pick a random
   `WEBHOOK_SECRET` (`openssl rand -hex 32`).
5. `docker compose --profile tunnel up -d --build`.

### VPS

Any host with TLS works. Point `WEBHOOK_URL` at your domain, run a reverse
proxy (caddy, nginx, traefik) terminating TLS in front of the bot's `:8080`,
keep the database on the same host or in the same private network.

### Switching deployments

Telegram knows only the webhook URL. Migrating between hosts is one
`setWebhook` call (the bot does this automatically on boot from `WEBHOOK_URL`)
plus a `pg_dump` / `pg_restore` for the database.

## Architecture

```
crates/
├── parser/    deterministic time-expression parser, DE + EN, fuzzy keywords
├── core/      domain types — User, Alert, AlertScope, RecurrencePattern
├── storage/   sqlx-backed Postgres repository
└── bot/       teloxide command handlers, callback queries, delivery worker
```

- **Webhook** is served by `teloxide`'s `webhooks::axum` listener with the
  Telegram secret-token header verified on every request.
- **Worker** sleeps until the earliest pending alert's `fire_at` or until
  Postgres `NOTIFY alerts_changed` wakes it. Claimed alerts cycle
  `pending → claimed → sent`; transient failures are rescheduled with
  exponential backoff, permanent ones (`BotBlocked`, `ChatNotFound`,
  `UserDeactivated`) are marked `failed`.
- **Reaper** unsticks claims left behind by crashed workers (default 60s
  timeout) and purges old `processed_updates` rows.

## Development

Requires Rust ≥ 1.86 and a running Postgres for integration testing.

```bash
cargo test --workspace      # parser unit tests + recurrence roundtrip
cargo run -p alert-bot      # local bot, needs DATABASE_URL + BOT_TOKEN in env
```

The parser is the most-fuzzed component — it lives in its own crate with no
async or I/O dependencies, so `cargo fuzz` can run against it directly.

## Roadmap

- **Planned maintenance windows** — schedule a downtime; alerts that would
  fire inside it get a heads-up notification before and/or after, instead of
  the catch-up-with-delay-note behaviour the worker uses for unplanned outages.
- **`/edit`** flow — left out of v0.1 because the Force-Reply UX felt clumsier
  than cancelling and recreating.
- **Per-chat rate limiting** via `governor` (already a dependency) for groups
  with bursty alert traffic.
- **Encrypted alert text at rest** if a real threat model shows up.

## License

MIT OR Apache-2.0
