#!/usr/bin/env bash
#
# Tail logs of a service on the production LXC.
#
# Usage:
#   ./scripts/logs.sh           # default: bot
#   ./scripts/logs.sh postgres
#   ./scripts/logs.sh bot 200   # last 200 lines

set -euo pipefail

LXC_HOST="${LXC_HOST:-root@10.0.70.240}"
REPO_PATH="${REPO_PATH:-/opt/alert-bot-rs}"
SERVICE="${1:-bot}"
TAIL_LINES="${2:-100}"

ssh -t "$LXC_HOST" \
    "cd '$REPO_PATH' && docker compose logs -f --tail=$TAIL_LINES $SERVICE"
