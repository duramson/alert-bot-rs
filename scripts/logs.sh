#!/usr/bin/env bash
#
# Tail logs of a systemd service on the production LXC.
#
# Usage:
#   ./scripts/logs.sh                       # default: alert-bot, last 100
#   ./scripts/logs.sh postgresql            # follow postgres logs
#   ./scripts/logs.sh alert-bot 500         # last 500 lines + follow
#   ./scripts/logs.sh alertbot-backup 50    # backup job's last 50 lines

set -euo pipefail

# Echte Ziel-IP steht in scripts/local.env (gitignored), nicht im Repo.
[ -f "$(dirname "$0")/local.env" ] && . "$(dirname "$0")/local.env"
LXC_HOST="${LXC_HOST:-root@<lxc-ip>}"
SERVICE="${1:-alert-bot}"
TAIL_LINES="${2:-100}"

exec ssh -t "$LXC_HOST" "journalctl -u $SERVICE -n $TAIL_LINES -f"
