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
| `POSTGRES_PASSWORD`  | yes      | —                    | also set in compose                                      |
| `POSTGRES_USER`      | no       | `alertbot`           |                                                          |
| `POSTGRES_DB`        | no       | `alertbot`           |                                                          |
| `ADMIN_CHAT_ID`      | no       | empty                | Telegram chat id that gets ops notifications (start, stop, worker errors) |
| `WEBHOOK_URL`        | no       | empty (long-polling) | public HTTPS URL Telegram will POST to                   |
| `WEBHOOK_LISTEN`     | no       | `0.0.0.0:8080`       | bind address inside the container                        |
| `WEBHOOK_SECRET`     | no       | empty                | recommended; sent as `X-Telegram-Bot-Api-Secret-Token`   |
| `CLOUDFLARED_TOKEN`  | no       | empty                | only needed for the `tunnel` profile                     |
| `RUST_LOG`           | no       | `info,alert_bot=debug` |                                                       |

## Production deployment (Proxmox LXC + Cloudflare Tunnel)

This is how the live instance runs. Topology:

```
Telegram  ──HTTPS──►  Cloudflare edge  ──tunnel──►  cloudflared (LXC)
                                                         │
                                                  http   ▼
                                                  ┌──────────────┐
                                                  │ Docker LXC   │
                                                  │              │
                                                  │   bot ◄──► postgres
                                                  └──────────────┘
                                                         │
                                                       SFTP nightly
                                                         ▼
                                                  Netcup webhosting (off-site dumps)
```

### One-time setup

#### 1. Create the LXC

On the Proxmox host:

```bash
bash -c "$(wget -qLO - https://github.com/community-scripts/ProxmoxVE/raw/main/ct/docker.sh)"
```

Choose Debian-12 unprivileged, defaults are fine (2 CPU / 2 GB / 8 GB disk),
nesting + keyctl + fuse get configured, Docker + compose-v2 preinstalled.

Note the LXC's IP — it shows at the end of the script.

#### 2. Clone the repo + configure env

```bash
ssh root@<lxc-ip>
gh auth login                                  # or git clone with HTTPS PAT
gh repo clone <you>/alert-bot-rs /opt/alert-bot-rs
cd /opt/alert-bot-rs
cp .env.example .env
nano .env
# Set: BOT_TOKEN, POSTGRES_PASSWORD, ADMIN_CHAT_ID
# Leave WEBHOOK_URL/WEBHOOK_SECRET empty for now (we test polling first)
```

#### 3. Bring up Postgres + bot in long-polling for sanity

```bash
docker compose up -d
docker compose logs -f bot
```

You should see `transport=polling` in the startup line and get a
`✅ alert-bot started` Telegram message in your DM (if `ADMIN_CHAT_ID` set).

#### 4. Migrate data from the old host (skip if starting fresh)

On the source machine:

```bash
docker compose stop bot
docker compose exec -T postgres \
    pg_dump -U alertbot --clean --if-exists alertbot > alertbot.sql
scp alertbot.sql root@<lxc-ip>:/opt/alert-bot-rs/
```

On the LXC:

```bash
docker compose exec -T postgres psql -U alertbot -d alertbot < alertbot.sql
docker compose exec postgres psql -U alertbot -d alertbot \
    -c "SELECT count(*), max(created_at) FROM alerts;"
docker compose restart bot
```

#### 5. Cloudflare Tunnel route + webhook switch

In the Cloudflare Zero Trust dashboard:
- Tunnel → Public Hostnames → Add
- Subdomain: `alert` (or any), domain: yours
- Service: HTTP, URL: `<lxc-ip>:8080`

Then on the LXC:

```bash
nano .env
# WEBHOOK_URL=https://alert.yourdomain.de/webhook
# WEBHOOK_SECRET=$(openssl rand -hex 32)
docker compose up -d --force-recreate bot

# verify
source .env
curl -s "https://api.telegram.org/bot${BOT_TOKEN}/getWebhookInfo" | jq
```

`url` should match your Cloudflare hostname, `pending_update_count` should be 0.

#### 6. Nightly off-site backup → Netcup SFTP

```bash
apt install -y curl sshpass
cp scripts/backup-postgres.sh /usr/local/bin/alertbot-backup.sh
cp scripts/systemd/alertbot-backup.{service,timer} /etc/systemd/system/
nano /etc/alertbot-backup.env
# COMPOSE_DIR=/opt/alert-bot-rs
# BACKUP_NETCUP_HOST=...
# BACKUP_NETCUP_USER=...
# BACKUP_NETCUP_PASS=...
# BACKUP_NETCUP_PATH=backups/alertbot
# RETENTION_DAYS=30
chmod 600 /etc/alertbot-backup.env
systemctl daemon-reload
systemctl enable --now alertbot-backup.timer
systemctl start alertbot-backup.service        # test now
journalctl -u alertbot-backup -n 30
```

Daily at 03:30 UTC the timer streams a gzipped dump to Netcup. Old files
(>30 days) get cleaned up automatically.

### Restoring from backup

```bash
docker compose up -d postgres
curl --user "$BACKUP_NETCUP_USER:$BACKUP_NETCUP_PASS" \
    "sftp://your-netcup-host/backups/alertbot/alertbot-2026-05-09T033000Z.sql.gz" \
  | gunzip \
  | docker compose exec -T postgres psql -U alertbot -d alertbot
docker compose up -d bot
```

`pg_dump --clean --if-exists` makes the restore idempotent — it drops and
recreates objects, so running it on a non-empty DB is safe.

## Update workflow

Local dev → push → LXC pulls + rebuilds. Single command from the dev machine:

```bash
./scripts/deploy.sh
```

What it does:
1. Runs `cargo test --workspace` (skip with `--no-test` for hotfixes)
2. `git push`
3. SSH into the LXC, `git pull --ff-only`, `docker compose up -d --build`
4. Tails bot logs for 30s so you see the startup / catch errors live

Config via env (export in your shell or `.env.local`):

```bash
export LXC_HOST=root@10.0.70.240
export REPO_PATH=/opt/alert-bot-rs
```

Other helpers:

```bash
./scripts/logs.sh           # tail bot logs (or pass another service: postgres)
./scripts/logs.sh bot 200   # last 200 lines + follow
```

### Rollback

```bash
ssh root@<lxc-ip>
cd /opt/alert-bot-rs
git log --oneline | head        # find the commit you want to go back to
git reset --hard <sha>
docker compose up -d --build
```

The Postgres data isn't touched by container rebuilds (separate volume), so
this is safe for any code-only rollback. For schema rollbacks you'd restore
from a `pg_dump` — keep your daily Netcup backups close.

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
