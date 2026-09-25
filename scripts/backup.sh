#!/usr/bin/env bash
# Back up the native install: database dump, service config and the data directory.
# The master key (/etc/rigdeck/master.key) is intentionally NOT included: store it separately.
#   sudo bash scripts/backup.sh [destination]
set -euo pipefail
cd "$(dirname "$0")/.."
umask 077
destination="${1:-backups/$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$destination"
destination="$(realpath "$destination")"
test ! -e "$destination/database.dump" || { printf 'Backup already exists; choose a new directory.\n' >&2; exit 1; }
set -a
# shellcheck disable=SC1091
. /etc/rigdeck/rigdeck.env
set +a
pg_dump "$DATABASE_URL" -Fc > "$destination/database.dump"
config_files=(etc/rigdeck/rigdeck.env etc/systemd/system/rigdeck.service)
[[ -f /etc/caddy/Caddyfile ]] && config_files+=(etc/caddy/Caddyfile)
tar -C / -cf "$destination/config.tar" "${config_files[@]}"
tar -C "${RIGDECK_DATA_DIR:-/var/lib/rigdeck-controller}" -cf "$destination/data.tar" .
sums=(database.dump config.tar data.tar)
if [[ -d /var/lib/caddy/.local/share/caddy ]]; then
  # Caddy's internal CA: losing it means re-importing the root certificate on every client.
  tar -C /var/lib/caddy/.local/share -cf "$destination/caddy.tar" caddy
  sums+=(caddy.tar)
fi
(cd "$destination" && sha256sum "${sums[@]}" > SHA256SUMS)
printf 'Backup saved to %s\nMaster key is intentionally separate: /etc/rigdeck/master.key\n' "$destination"
