#!/usr/bin/env bash
# Expresso — PostgreSQL daily backup.
# Reads PG* env from /etc/default/expresso-pg-backup.
# pg_dump custom format → /var/backups/expresso/pg/expresso-<UTC>.dump
# Retention: delete backups older than RETENTION_DAYS (default 30).
set -euo pipefail

: "${PGHOST:?missing}"
: "${PGPORT:?missing}"
: "${PGUSER:?missing}"
: "${PGDATABASE:?missing}"
: "${PGPASSWORD:?missing}"
BACKUP_DIR="${BACKUP_DIR:-/var/backups/expresso/pg}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"

mkdir -p "$BACKUP_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="$BACKUP_DIR/expresso-${STAMP}.dump"
TMP="${OUT}.part"

# pg_dump via docker → no need for host postgresql-client.
docker run --rm \
  -e PGHOST -e PGPORT -e PGUSER -e PGDATABASE -e PGPASSWORD \
  -v "$BACKUP_DIR:/backup" \
  postgres:16-alpine \
  pg_dump -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" -d "$PGDATABASE" \
          -Fc -Z 6 --no-owner --no-acl \
          -f "/backup/$(basename "$TMP")"

mv "$TMP" "$OUT"
chmod 600 "$OUT"

# Verify archive integrity → list contents.
docker run --rm -v "$BACKUP_DIR:/backup" postgres:16-alpine \
  pg_restore -l "/backup/$(basename "$OUT")" >/dev/null

# Retention.
find "$BACKUP_DIR" -maxdepth 1 -name 'expresso-*.dump' -mtime "+${RETENTION_DAYS}" -delete

# Log size to journald.
SIZE="$(stat -c%s "$OUT")"
echo "backup ok: $OUT ($SIZE bytes)"
