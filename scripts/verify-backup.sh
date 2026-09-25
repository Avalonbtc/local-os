#!/usr/bin/env bash
# Restore a dump into a throw-away database and count key records; the controller never sees it.
#   sudo bash scripts/verify-backup.sh backups/<dir>/database.dump
set -euo pipefail
dump="$(realpath "${1:?usage: verify-backup.sh backup/database.dump}")"
test -s "$dump"
database="rigdeck_restore_$(date +%s)_$$"
[[ "$database" =~ ^rigdeck_restore_[0-9]+_[0-9]+$ ]]
cleanup() { sudo -u postgres dropdb --if-exists "$database"; }
trap cleanup EXIT
sudo -u postgres createdb "$database"
sudo -u postgres pg_restore --exit-on-error --no-owner -d "$database" < "$dump"
sudo -u postgres psql -d "$database" -v ON_ERROR_STOP=1 -c "SELECT 'machines' AS entity,count(*) FROM machines UNION ALL SELECT 'jobs',count(*) FROM jobs UNION ALL SELECT 'audit',count(*) FROM audit_events;"
printf 'Isolated restore validation passed; no controller was started against restored jobs.\n'
