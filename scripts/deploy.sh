#!/usr/bin/env bash
#
# Deploy the local working copy to the production LXC.
#
# Workflow:
#   1. Optional sanity checks (cargo test, clippy)
#   2. git push to origin
#   3. SSH into LXC → git pull --ff-only → docker compose up -d --build
#   4. Tail bot logs briefly so we see startup output
#
# Config via env (export in your shell or put in `.envrc`/.env.local):
#   LXC_HOST    SSH target, default root@10.0.70.240
#   REPO_PATH   where the repo lives on the LXC, default /opt/alert-bot-rs
#
# Flags:
#   --no-test   skip `cargo test` (use sparingly — emergency hotfix only)
#   --no-tail   don't tail logs after deploy (return immediately)

set -euo pipefail

LXC_HOST="${LXC_HOST:-root@10.0.70.240}"
REPO_PATH="${REPO_PATH:-/opt/alert-bot-rs}"
SKIP_TESTS=0
TAIL=1
LOG_SECS=30

for arg in "$@"; do
    case "$arg" in
        --no-test) SKIP_TESTS=1 ;;
        --no-tail) TAIL=0 ;;
        -h|--help)
            grep -E '^# ' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *)
            echo "unknown flag: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done

# Warn (don't block) on uncommitted local changes — they'd be left behind.
if ! git diff-index --quiet HEAD --; then
    echo "⚠️  uncommitted changes won't be deployed:"
    git status --short | head -20
    echo
fi

if [ $SKIP_TESTS -eq 0 ]; then
    echo "▶ cargo test --workspace"
    cargo test --workspace --quiet
fi

echo "▶ git push"
git push

echo "▶ deploying to $LXC_HOST:$REPO_PATH"
ssh "$LXC_HOST" "set -e
    cd '$REPO_PATH'
    git pull --ff-only
    docker compose up -d --build
    docker compose ps"

if [ $TAIL -eq 1 ]; then
    echo "▶ tailing bot logs for ${LOG_SECS}s (Ctrl-C to stop early)"
    ssh -t "$LXC_HOST" "cd '$REPO_PATH' && timeout $LOG_SECS docker compose logs -f --tail=20 bot" || true
fi

echo "✅ deployed"
