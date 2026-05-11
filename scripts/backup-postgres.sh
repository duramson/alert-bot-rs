#!/usr/bin/env bash
#
# Daily Postgres backup → Netcup SFTP. Native (no docker).
#
# Runs as the `postgres` system user via the systemd unit, so peer
# authentication on the local Unix socket gives us full access to every
# database without a password in /etc/alertbot-backup.env.
#
# pg_dump --clean --if-exists makes restores idempotent: re-running on
# an existing schema drops and recreates objects.
#
# Required env (provide via /etc/alertbot-backup.env):
#   BACKUP_NETCUP_HOST    e.g. yourhost.netcup.net
#   BACKUP_NETCUP_USER    SFTP username
#   BACKUP_NETCUP_PASS    SFTP password
#
# Optional env:
#   BACKUP_NETCUP_PATH    remote directory (default: backups/alertbot)
#   DB_NAME               postgres database to dump (default: alertbot)
#   RETENTION_DAYS        prune remote dumps older than N days (default: 30)
#
# Required packages on the host: postgresql-client, curl, sshpass

set -euo pipefail

: "${BACKUP_NETCUP_HOST:?must be set}"
: "${BACKUP_NETCUP_USER:?must be set}"
: "${BACKUP_NETCUP_PASS:?must be set}"
: "${BACKUP_NETCUP_PATH:=backups/alertbot}"
: "${DB_NAME:=alertbot}"
: "${RETENTION_DAYS:=30}"

DATE=$(date -u +%Y-%m-%dT%H%M%SZ)
REMOTE_FILE="${BACKUP_NETCUP_PATH}/alertbot-${DATE}.sql.gz"

echo "[backup] dumping ${DB_NAME} → ${REMOTE_FILE}"

pg_dump --clean --if-exists --no-owner --no-privileges "${DB_NAME}" \
  | gzip -9 \
  | curl --silent --show-error --fail \
         --user "${BACKUP_NETCUP_USER}:${BACKUP_NETCUP_PASS}" \
         --upload-file - \
         --ftp-create-dirs \
         "sftp://${BACKUP_NETCUP_HOST}/${REMOTE_FILE}"

echo "[backup] uploaded"

# Retention: delete remote files older than RETENTION_DAYS. Netcup SFTP
# accepts a single sftp batch — we list, then issue rm for each match.
CUTOFF_EPOCH=$(date -u -d "${RETENTION_DAYS} days ago" +%s 2>/dev/null \
              || date -u -v-"${RETENTION_DAYS}d" +%s)

OLD_FILES=$(sshpass -p "${BACKUP_NETCUP_PASS}" \
    sftp -q -o StrictHostKeyChecking=accept-new \
         -b <(printf 'cd %s\nls -1\nbye\n' "${BACKUP_NETCUP_PATH}") \
         "${BACKUP_NETCUP_USER}@${BACKUP_NETCUP_HOST}" 2>/dev/null \
  | grep -E '^alertbot-[0-9TZ]+\.sql\.gz$' || true)

for f in $OLD_FILES; do
    # extract YYYY-MM-DDTHHMMSSZ from filename
    ts=$(echo "$f" | sed -E 's/^alertbot-(.+)\.sql\.gz$/\1/')
    file_epoch=$(date -u -d "${ts:0:10} ${ts:11:2}:${ts:13:2}:${ts:15:2}" +%s 2>/dev/null || true)
    if [[ -n "$file_epoch" && "$file_epoch" -lt "$CUTOFF_EPOCH" ]]; then
        echo "[backup] deleting ${f} (older than ${RETENTION_DAYS} days)"
        sshpass -p "${BACKUP_NETCUP_PASS}" \
            sftp -q -o StrictHostKeyChecking=accept-new \
                 -b <(printf 'cd %s\nrm %s\nbye\n' "${BACKUP_NETCUP_PATH}" "$f") \
                 "${BACKUP_NETCUP_USER}@${BACKUP_NETCUP_HOST}" >/dev/null
    fi
done

echo "[backup] done"
