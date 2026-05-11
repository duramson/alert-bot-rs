# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

A Telegram alert/reminder bot written in Rust. Users send commands like `/alert 5m coffee ready` or `/alert do 14:00 Standup`; the bot parses German/English time expressions, persists alerts in Postgres, and delivers them at fire time. Default audience and fallback language is German.

## Workspace layout

Cargo workspace with four crates (`crates/`):

- **`parser`** — pure-function time-expression parser. No I/O, no DB. Parses input through three strategies in order: relative (`5m`, `in 2 stunden`), absolute date (`30.4.26`, `30.04.2027 14:30`), named day (`morgen`, `do`/`thu`). Keyword matching uses `strsim::levenshtein` with an adaptive threshold so typos work, but fuzzy matching applies *only* to time keywords — never to the reminder text. Default time when only a date is given is 09:00 local (`DEFAULT_TIME`).
- **`core`** (package name `alert-bot-core`, imported as `botcore`) — domain types shared by all other crates: `Alert`, `User`, `ChatType`, `AlertScope`, `AlertState`, `Language`, plus the `Recurrence` trait. Enum values are persisted as their `as_str()` form so adding a Telegram chat type doesn't need a migration. Recurrence types exist but v1 only handles one-shot alerts.
- **`storage`** (package name `alert-bot-storage`) — Postgres layer wrapping `sqlx::PgPool`. Uses runtime `sqlx::query` (not the `query!` macro) so the workspace builds without a live DB. Owns all SQL and row→domain mapping. `PgStore::migrate` runs migrations from `migrations/`.
- **`bot`** — the binary (`alert-bot`). Wires teloxide command dispatch, the delivery worker, and config from env. `main.rs` is the only place that reads env vars or constructs the `Bot`.

Internal crate deps are declared in the workspace `Cargo.toml` so individual crates can `parser.workspace = true` etc.

## Architecture

### Two long-running tasks
1. **Telegram dispatcher** (`bot/handlers.rs`) — routes `/alert`, `/galert`, `/list`, `/cancel`, `/tz`, `/lang`, `/start`, `/help`. `dispatch` fetches/upserts the user, then delegates per command. Group-only `/galert` is rejected from DMs. `Alert::can_edit` (in `core`) is the single source of truth for edit/delete permissions.
2. **Delivery worker** (`bot/worker.rs`) — runs in a `tokio::spawn`d task. Loop:
   - Compute next wake = `MIN(fire_at) WHERE state = 'pending'` via `next_pending_fire_at`.
   - `tokio::select!` between: that timestamp, a Postgres `NOTIFY alerts_changed` (so newly created alerts wake the worker immediately), or shutdown.
   - `claim_due_alerts` atomically transitions rows `pending → claimed` using `FOR UPDATE SKIP LOCKED`, so multiple worker instances are safe.
   - Send via Telegram, then `mark_sent` / `mark_failed` / `reschedule`.
   - 429 `RetryAfter` → reschedule at `now + retry_after + 1s`. Permanent errors (`BotBlocked`, `ChatNotFound`, `UserDeactivated`) → `mark_failed`. Other transient errors retry with exponential backoff (`5 << attempts`) up to `MAX_ATTEMPTS = 5`.
   - A separate **reaper task** (`reaper_loop`) every 30s releases `claimed` rows older than `STALE_CLAIM_SECS = 60` (handles worker crashes mid-send) and purges `processed_updates` older than 24h.

### Postgres schema (`migrations/0001_init.sql`)
- `users` keyed by `telegram_id`. `upsert_user` is intentionally non-clobbering: it only bumps `updated_at`, never overwrites the user-set `timezone`/`language`.
- `alerts` with state machine `pending → claimed → sent | failed | cancelled`. Two partial indexes: `alerts_due_idx` on `fire_at WHERE state = 'pending'` (worker hot path) and `alerts_chat_idx WHERE state IN ('pending', 'claimed')` (`/list`).
- `processed_updates` for Telegram update_id idempotency (reaper purges).
- A trigger on `alerts` (INSERT or UPDATE OF `fire_at`, `state`) calls `pg_notify('alerts_changed', '')` to wake the worker without polling.

### Transport
`main.rs` picks **webhook** (axum, via `teloxide::update_listeners::webhooks`) when `WEBHOOK_URL` is set, otherwise **long polling** for local dev. The webhook listener binds to `WEBHOOK_LISTEN` (default `0.0.0.0:8080`).

## Commands

Cargo workspace — every command is run from the repo root.

```bash
cargo build                            # build all crates
cargo build --release --bin alert-bot  # binary used in Dockerfile
cargo test                             # all tests across the workspace
cargo test -p parser                   # only the parser crate
cargo test -p parser rel_minutes_short # single test by name
cargo check --workspace                # fast type-check, no codegen
cargo clippy --workspace --all-targets # lints
cargo fmt
```

The parser has the bulk of the unit tests (in `crates/parser/src/lib.rs`), with a fixed reference time of 2026-05-08 12:00 Europe/Berlin so date assertions are deterministic. There are no integration tests against Postgres yet — when adding any, prefer per-test transactional rollback over teardown SQL.

### Running locally

```bash
cp .env.example .env                # set BOT_TOKEN, POSTGRES_PASSWORD
docker compose up -d postgres       # just the DB
DATABASE_URL=postgres://alertbot:<pw>@localhost:5432/alertbot \
  BOT_TOKEN=... cargo run -p alert-bot
```

Migrations run automatically on startup (`PgStore::migrate` in `main.rs`). With no `WEBHOOK_URL` the bot uses long polling — fine for dev.

### Running in Docker

```bash
docker compose up -d                              # bot + postgres
docker compose --profile tunnel up -d             # also start cloudflared
docker compose logs -f bot
```

The `cloudflared` service is gated behind the `tunnel` profile and exposes the webhook port via Cloudflare Tunnel; `WEBHOOK_URL` in `.env` must point at the Cloudflare hostname.

## Production deployment

The live instance runs on a Proxmox-hosted Debian-12 LXC (created via the community-scripts `docker.sh` helper). `docker compose` brings up the bot + Postgres; the bot publishes `:8080` on the LXC's host network (Postgres stays internal). A separate cloudflared LXC on the same Proxmox host fronts a Cloudflare Tunnel that routes a public hostname to `<bot-lxc-ip>:8080` so Telegram can POST webhooks. The repo lives at `/opt/alert-bot-rs` on the bot LXC.

Off-site backups: `scripts/backup-postgres.sh` runs nightly via a systemd timer (`alertbot-backup.timer` → 03:30 UTC), streaming `pg_dump | gzip | SFTP` straight to a Netcup webhosting target. Config in `/etc/alertbot-backup.env` (chmod 600). Retention: 30 days, enforced by the script itself (lists remote files, deletes anything older than `RETENTION_DAYS`).

## Update workflow

Two coexisting paths:

**CI/CD (preferred for production)** — `.github/workflows/deploy.yml`. On push to `main`: `cargo test`, build the image with `docker buildx`, push to `ghcr.io/<you>/alert-bot-rs:{latest, sha-XXXXX}`, then SSH into the LXC via Cloudflare Access SSH (no public SSH port) and run `git pull && docker compose pull bot && docker compose up -d`. Required GH secrets: `LXC_SSH_KEY`, `CF_ACCESS_CLIENT_ID`, `CF_ACCESS_CLIENT_SECRET`. GH variables: `SSH_HOST`, `SSH_USER`. The LXC must have `COMPOSE_FILE=docker-compose.yml:docker-compose.prod.yml` in its `.env` so compose layers the prod override (which swaps the local `build:` for `image: ghcr.io/...`). Compose's auto-loading of `.env` picks this up — no `-f` flags needed. README has the full one-time setup walkthrough in the "CI/CD" section.

**Local fast path** — `./scripts/deploy.sh`. Pipeline: `cargo test --workspace` (skippable with `--no-test`), `git push`, SSH into the LXC, `git pull --ff-only`, `docker compose up -d --build` (builds locally on the LXC), tails bot logs for 30 s. Use this for tight iteration when CI feels slow. Requires `LXC_HOST` (default `root@10.0.70.240`) and `REPO_PATH` (default `/opt/alert-bot-rs`) exported in the dev shell.

`scripts/logs.sh [service] [tail]` is the quick remote tail. For rollbacks: `git reset --hard <sha>` on the LXC and either `docker compose up -d --build` (local rebuild) or `docker compose pull && up -d` (last image with that sha is still in ghcr.io). Postgres volume survives container rebuilds, so code-only rollbacks are safe; schema rollbacks need a Netcup-backup restore.

## Conventions worth knowing

- **`sqlx::query` not `query!`.** The workspace deliberately avoids compile-time-checked queries so it builds without a live DB. If migrating to `query!`, also add a `.sqlx/` offline cache and CI step to refresh it.
- **Domain enums persist as their `as_str()` form**, never as Postgres enum types — keeps schema migrations cheap when Telegram adds a new chat kind.
- **Permissions live on the domain type** (`Alert::can_edit`), not in SQL. `cancel_alert` enforces them in Rust before issuing the UPDATE.
- **Default language is German** (`Language::from_telegram_code` falls back to `De`). User-facing strings in `bot/messages.rs` are keyed off `Language` with plain functions, not Fluent.
- **Times in DB are always UTC** (`TIMESTAMPTZ`). Conversion to/from the user's `Tz` happens in `parser::local_to_utc` (DST-aware: ambiguous → earlier, gap → bump forward) and `bot/render.rs`.
- **Telegram HTML parse mode** is used for confirmation/help replies — escape user-controlled strings via the `html_escape` helper in `bot/handlers.rs` when interpolating into messages.
- **No `<code>` or `<pre>` tags in user-facing Telegram strings.** Telegram renders monospace fonts much larger than the surrounding body text, which looks broken on mobile. Use `<b>`, `<i>`, and plain text only. This applies to every string sent to Telegram — replies, help text, error messages, admin notifications. Inline command examples should be plain (`/alert 5m text`), not wrapped in `<code>`.
- **Multi-instance is supported** by design (`SKIP LOCKED` claim + idempotent `processed_updates`), but Telegram only delivers updates to one webhook — to actually run >1 replica you'd need a load-balancer in front of the webhook port.

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **alert-bot-rs** (492 symbols, 1138 relationships, 42 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `npx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/alert-bot-rs/context` | Codebase overview, check index freshness |
| `gitnexus://repo/alert-bot-rs/clusters` | All functional areas |
| `gitnexus://repo/alert-bot-rs/processes` | All execution flows |
| `gitnexus://repo/alert-bot-rs/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->
