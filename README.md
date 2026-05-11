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

## Quick start (local development)

```bash
git clone <your-fork>
cd alert-bot-rs
cp .env.example .env
# edit .env: BOT_TOKEN (from @BotFather) and POSTGRES_PASSWORD

docker compose up -d postgres       # local-dev DB only — bound to 127.0.0.1:5432
cargo run -p alert-bot
```

The bot falls back to long-polling when `WEBHOOK_URL` is unset, so no public
endpoint is needed for development. Open Telegram, message your bot, send
`/start`.

> **Use a separate dev bot.** Telegram delivers every update to a single
> webhook/poll target, so running this against your production bot token would
> fight the deployed instance for ownership. Create a second bot at
> [@BotFather](https://t.me/BotFather) and put its token in `.env`.

## Configuration

| Variable             | Required | Default                  | Notes                                                    |
|----------------------|----------|--------------------------|----------------------------------------------------------|
| `BOT_TOKEN`          | yes      | —                        | from [@BotFather](https://t.me/BotFather)                |
| `DATABASE_URL`       | yes      | —                        | `postgres://user:pw@host:port/db` (TCP) or `postgres:///alertbot?host=/var/run/postgresql` (Unix socket on the LXC) |
| `POSTGRES_PASSWORD`  | dev      | —                        | only consumed by `docker-compose.yml`; ignored in prod   |
| `POSTGRES_USER`      | dev      | `alertbot`               | same                                                     |
| `POSTGRES_DB`        | dev      | `alertbot`               | same                                                     |
| `ADMIN_CHAT_ID`      | no       | empty                    | Telegram chat id that gets ops notifications             |
| `WEBHOOK_URL`        | no       | empty (long-polling)     | public HTTPS URL Telegram POSTs to                       |
| `WEBHOOK_LISTEN`     | no       | `0.0.0.0:8080`           | bind address inside the process                          |
| `WEBHOOK_SECRET`     | no       | empty                    | recommended; sent as `X-Telegram-Bot-Api-Secret-Token`   |
| `RUST_LOG`           | no       | `info,alert_bot=debug`   |                                                          |

## Production deployment (Proxmox LXC + Cloudflare Tunnel)

This is how the live instance runs. cloudflared lives in its own LXC and
fronts multiple services on the same Proxmox host; the alert-bot LXC just
exposes `:8080` on the internal Proxmox bridge so cloudflared can reach it.

```
Telegram  ──HTTPS──►  Cloudflare edge  ──tunnel──►  cloudflared LXC
                                                         │ HTTP :8080
                                                         ▼
                                                  ┌──────────────────┐
                                                  │ alert-bot LXC    │
                                                  │  ┌────────────┐  │
                                                  │  │ alert-bot  │──┼──► postgres (local Unix socket)
                                                  │  └────────────┘  │
                                                  │   systemd-managed │
                                                  └──────────────────┘
                                                         │
                                                       SFTP nightly
                                                         ▼
                                                  Netcup webhosting (off-site dumps)
```

No Docker. Postgres and the bot binary both run as native systemd units on
the same LXC.

### One-time setup

#### 1. Create the LXC

On the Proxmox host, use the community-scripts PostgreSQL helper — it
gives you a Debian-12 LXC with Postgres 16 pre-installed and the standard
`local all all peer` line in `pg_hba.conf`:

```bash
bash -c "$(wget -qLO - https://community-scripts.org/scripts/postgresql)"
```

Pick the non-Alpine variant. The size difference is irrelevant on a
dedicated DB LXC and Debian's glibc matches what GitHub Actions produces
for the bot binary. Defaults are fine (2 CPU / 2 GB / 8 GB disk).

Note the LXC's IP — the script prints it at the end.

#### 2. Postgres role + database + system user

```bash
ssh root@<lxc-ip>

# System user the bot will run as. No shell, no home, no password —
# only systemd ever activates it.
useradd --system --no-create-home --shell /usr/sbin/nologin alertbot

# Postgres role that matches the system user, so peer auth on the
# local Unix socket Just Works.
sudo -u postgres createuser alertbot
sudo -u postgres createdb -O alertbot alertbot

# Sanity check — should print a row count of 0.
sudo -u alertbot psql -d alertbot -c "SELECT count(*) FROM pg_tables WHERE schemaname='public';"
```

No password is stored anywhere — peer auth via the Unix socket binds
identity to the calling system user.

#### 3. Install the systemd unit + config file

The repo lives only on your dev machine; the LXC only needs the unit
files and a config file. Copy them over the first time:

```bash
# from your dev machine
scp scripts/systemd/alert-bot.service       root@<lxc-ip>:/etc/systemd/system/
scp scripts/systemd/alertbot-backup.service root@<lxc-ip>:/etc/systemd/system/
scp scripts/systemd/alertbot-backup.timer   root@<lxc-ip>:/etc/systemd/system/
scp scripts/backup-postgres.sh              root@<lxc-ip>:/usr/local/bin/alertbot-backup.sh
```

```bash
# on the LXC
mkdir -p /etc/alert-bot
cat > /etc/alert-bot/config.env <<EOF
BOT_TOKEN=<from BotFather>
DATABASE_URL=postgres:///alertbot?host=/var/run/postgresql
ADMIN_CHAT_ID=<your numeric Telegram user id>
RUST_LOG=info,alert_bot=debug
# WEBHOOK_URL=         # added in step 5
# WEBHOOK_SECRET=      # added in step 5
EOF
chmod 600 /etc/alert-bot/config.env
chown -R alertbot:alertbot /etc/alert-bot

chmod +x /usr/local/bin/alertbot-backup.sh
systemctl daemon-reload
```

The bot isn't installed yet — that's step 4.

#### 4. Install the first binary

Until CI is wired up (step 7), build locally on the Mac and scp the
binary across. Releases on GitHub stay private with the repo, so the
LXC can't `curl` them without auth — CI gets around this by scp'ing
from the runner directly (step 7).

```bash
# on the Mac
cargo build --release --bin alert-bot
strip target/release/alert-bot
scp target/release/alert-bot root@<lxc-ip>:/tmp/alert-bot
```

```bash
# on the LXC
install -m 0755 -o root -g root /tmp/alert-bot /usr/local/bin/alert-bot
rm /tmp/alert-bot

systemctl enable --now alert-bot
systemctl status alert-bot --no-pager -l
journalctl -u alert-bot -n 30
```

You should see `transport=polling` in the startup line. If you set
`ADMIN_CHAT_ID`, your Telegram DM gets a `✅ alert-bot started`
notification.

Migrations run automatically on every start (`PgStore::migrate`), so the
schema is materialised on first boot.

> **glibc note.** Building the release locally on macOS produces a
> Mach-O binary that won't run on Linux. If you're on macOS, either use
> `cargo zigbuild --target x86_64-unknown-linux-gnu --release` (needs
> `brew install zig && cargo install cargo-zigbuild`), or just push the
> commit and let the CI workflow do the build for you — the workflow
> scp's straight to the LXC after build.

#### 5. Cloudflare Tunnel route → webhook switch

In the Cloudflare Zero Trust dashboard for the existing cloudflared LXC:

- Tunnel → Public Hostnames → Add
- Subdomain: `alert` (or whatever), Domain: yours
- Service: HTTP, URL: `<bot-lxc-ip>:8080`

Then on the LXC:

```bash
nano /etc/alert-bot/config.env
# WEBHOOK_URL=https://alert.yourdomain.de/webhook
# WEBHOOK_SECRET=$(openssl rand -hex 32)

systemctl restart alert-bot

# verify
source /etc/alert-bot/config.env
curl -s "https://api.telegram.org/bot${BOT_TOKEN}/getWebhookInfo" | jq
```

`url` should match your Cloudflare hostname, `pending_update_count`
should be `0`.

#### 6. Migrate data from the old host (skip if starting fresh)

On the source machine (old docker-on-LXC setup):

```bash
docker compose stop bot
docker compose exec -T postgres \
    pg_dump -U alertbot --clean --if-exists --no-owner --no-privileges alertbot \
    > alertbot.sql
scp alertbot.sql root@<new-lxc-ip>:/tmp/
```

On the new LXC:

```bash
systemctl stop alert-bot
sudo -u postgres psql -d alertbot < /tmp/alertbot.sql
sudo -u postgres psql -d alertbot \
    -c "SELECT count(*), max(created_at) FROM alerts;"
systemctl start alert-bot
```

#### 7. Cloudflare Access SSH for CI deploys

The GitHub Actions workflow connects to the LXC over Cloudflare Access
SSH — no SSH port needs to be exposed on the LXC.

**Generate a dedicated key pair** (don't reuse your personal one):

```bash
ssh-keygen -t ed25519 -f ~/.ssh/alertbot-deploy -C "github-actions-deploy"
ssh-copy-id -i ~/.ssh/alertbot-deploy.pub root@<lxc-ip>
```

**Cloudflare Tunnel → SSH route + Access policy.** Same cloudflared
instance as the webhook route:

- Public Hostnames → Add
  - Subdomain: `ssh-alert` (or anything)
  - Service: **SSH**, URL: `<bot-lxc-ip>:22`
- Zero Trust → Access → Applications → Add Application
  - **Self-hosted**, App URL: `ssh-alert.yourdomain.de`
  - Add a policy: action **Allow**, **Service Auth** rule → create a
    fresh Service Token, copy the Client ID and Client Secret (shown
    only once)

**GitHub repository secrets/variables.** Settings → Secrets and variables
→ Actions:

| Type     | Name                          | Value                                    |
|----------|-------------------------------|------------------------------------------|
| Secret   | `LXC_SSH_KEY`                 | contents of `~/.ssh/alertbot-deploy`     |
| Secret   | `CF_ACCESS_CLIENT_ID`         | from the Service Token                   |
| Secret   | `CF_ACCESS_CLIENT_SECRET`     | from the Service Token                   |
| Variable | `SSH_HOST`                    | `ssh-alert.yourdomain.de`                |
| Variable | `SSH_USER`                    | `root`                                   |

The next push to `master` triggers the workflow → build → publish to
the `latest` release → SSH-deploy via Cloudflare Access.

#### 8. Nightly off-site backup → Netcup SFTP

```bash
apt install -y curl sshpass postgresql-client

cat > /etc/alertbot-backup.env <<EOF
BACKUP_NETCUP_HOST=yourhost.netcup.net
BACKUP_NETCUP_USER=<sftp user>
BACKUP_NETCUP_PASS=<sftp password>
BACKUP_NETCUP_PATH=backups/alertbot
RETENTION_DAYS=30
EOF
chmod 600 /etc/alertbot-backup.env

systemctl daemon-reload
systemctl enable --now alertbot-backup.timer
systemctl start alertbot-backup.service        # test now
journalctl -u alertbot-backup -n 30
```

Daily at 03:30 UTC the timer streams a gzipped `pg_dump` to Netcup. The
script prunes anything older than `RETENTION_DAYS` on the remote side.

### Restoring from backup

```bash
systemctl stop alert-bot
curl --user "$BACKUP_NETCUP_USER:$BACKUP_NETCUP_PASS" \
    "sftp://your-netcup-host/backups/alertbot/alertbot-2026-05-09T033000Z.sql.gz" \
  | gunzip \
  | sudo -u postgres psql -d alertbot
systemctl start alert-bot
```

`pg_dump --clean --if-exists` makes the restore idempotent — it drops
and recreates objects, so running it on a non-empty DB is safe.

## Update workflow (CI/CD)

Push to `master` → GitHub Actions builds → runner scp's straight to the
LXC and restarts. The full pipeline is in
[`.github/workflows/deploy.yml`](.github/workflows/deploy.yml):

1. **Test** — `cargo test --workspace`
2. **Build & deploy** (one job, so the runner can scp the binary it just
   built):
   1. `cargo build --release --bin alert-bot` on `ubuntu-22.04`.
      Pinned so the binary's glibc baseline (2.35) stays compatible
      with Debian 13 (2.41 — backward-compatible). If we let
      `ubuntu-latest` drift past Debian, deploy silently breaks.
   2. `strip target/release/alert-bot`
   3. `scp` the binary to the LXC over Cloudflare Access SSH.
   4. On the LXC: `install -m 0755` to `/usr/local/bin/alert-bot`,
      then `systemctl restart alert-bot`.
3. **Tail** — 20s of `journalctl -u alert-bot -f` so a broken startup
   surfaces in the workflow output.

No GitHub Release roundtrip — keeps the repo free to stay private.

### Manual trigger

The workflow has `workflow_dispatch` so you can re-deploy any commit
from the Actions tab without pushing — useful for retrying a flaky
deploy or re-applying after a Cloudflare config change.

### Tail logs from your dev machine

```bash
./scripts/logs.sh                       # default: alert-bot, last 100 + follow
./scripts/logs.sh postgresql            # postgres logs
./scripts/logs.sh alertbot-backup 50    # last backup run
```

Requires `LXC_HOST` (default `root@10.0.70.240`) exported in your shell.

### Rollback

Trigger the workflow on an older commit:

- GitHub → Actions → "Build and deploy to production" → **Run
  workflow** → pick a branch or tag → run.

That rebuilds + scp's the older commit's binary. Or build locally and
scp manually (same one-liner as step 4 in the production setup above).

Postgres data isn't touched by binary swaps, so code-only rollbacks are
safe. Schema rollbacks need a `pg_dump` restore — keep your daily Netcup
backups close.

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
- **Update idempotency** — every Telegram `update_id` is recorded in
  `processed_updates` at the top of the dispatcher, so retried webhooks
  don't cause duplicate deliveries.

## Development

Requires Rust ≥ 1.86.

```bash
cargo test --workspace      # parser unit tests + recurrence roundtrip
cargo run -p alert-bot      # local bot, needs DATABASE_URL + BOT_TOKEN in env
cargo clippy --workspace --all-targets
cargo fmt
```

The parser is the most-fuzzed component — it lives in its own crate with no
async or I/O dependencies, so `cargo fuzz` can run against it directly.

## Roadmap

- **Planned maintenance windows** — schedule a downtime; alerts that would
  fire inside it get a heads-up notification before and/or after, instead of
  the catch-up-with-delay-note behaviour the worker uses for unplanned outages.
- **`/edit`** flow — left out of v0.1 because the Force-Reply UX felt clumsier
  than cancelling and recreating.
- **Per-chat rate limiting** for groups with bursty alert traffic.
- **Encrypted alert text at rest** if a real threat model shows up.

## License

MIT OR Apache-2.0
