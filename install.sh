#!/usr/bin/env bash
# Fresh native installation. Run from a downloaded file, never from a partial pipe.
set -euo pipefail
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }
mode=https host=rigdeck.local listen=0.0.0.0 destination=/opt/local-os
owner="${SUDO_USER:-rigdeck}"; [[ "$owner" != root ]] || owner=rigdeck
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode|--host|--listen|--dir|--user)
      [[ $# -ge 2 ]] || die "Missing value for $1"
      case "$1" in
        --mode) mode="$2";; --host) host="$2";; --listen) listen="$2";;
        --dir) destination="$2";; --user) owner="$2";;
      esac; shift 2;;
    -h|--help)
      echo 'sudo bash install.sh [--mode https|tunnel|cloudflared] [--host HOST] [--listen IPv4] [--dir /opt/local-os] [--user USER]'
      echo 'Fresh installations only. Prompts for an admin password on the terminal; existing installations use scripts/native/restart.sh.'
      exit 0;;
    *) die "Unknown option: $1";;
  esac
done
[[ $EUID -eq 0 ]] || die 'Run with sudo bash install.sh'
[[ "$mode" =~ ^(https|tunnel|cloudflared)$ ]] || die 'Invalid mode'
[[ "$host" =~ ^[A-Za-z0-9.-]+$ && "$listen" =~ ^[0-9.]+$ ]] || die 'Invalid host/listen address'
[[ "$destination" =~ ^/[A-Za-z0-9._/-]+$ && "$destination" != / && "$destination" != *'/../'* && "$destination" != */.. ]] || die 'Use a simple absolute installation path'
[[ "$owner" =~ ^[a-z_][a-z0-9_.-]*$ && "$owner" != root ]] || die 'Invalid non-root service user'
. /etc/os-release
case "$ID:$VERSION_ID" in ubuntu:22.04|ubuntu:24.04|debian:12) ;; *) die 'Supported: Ubuntu 22.04/24.04 and Debian 12';; esac
[[ -d /run/systemd/system ]] || die 'A running systemd host is required'
[[ ! -e "$destination" && ! -L "$destination" ]] || die "Destination already exists: $destination (nothing overwritten)"
[[ ! -e /etc/rigdeck/rigdeck.env && ! -e /etc/rigdeck/master.key && ! -e /var/lib/rigdeck-controller ]] || die 'Existing installation detected; use its restart.sh instead'
if command -v psql >/dev/null && id postgres >/dev/null 2>&1; then
  existing="$(runuser -u postgres -- psql -Atc "SELECT count(*) FROM pg_database WHERE datname='rigdeck'; SELECT count(*) FROM pg_roles WHERE rolname='rigdeck'")" || die 'Cannot inspect existing PostgreSQL; no changes made'
  [[ "$existing" == $'0\n0' ]] || die 'Existing rigdeck database or role; no changes made'
fi
[[ -r /dev/tty && -w /dev/tty ]] || die 'An interactive terminal is required to set the administrator password'
read -rsp 'Admin password (at least 12 characters): ' password < /dev/tty; printf '\n' > /dev/tty
read -rsp 'Confirm password: ' confirmation < /dev/tty; printf '\n' > /dev/tty
[[ ${#password} -ge 12 && "$password" == "$confirmation" ]] || die 'Passwords must match and contain at least 12 characters'
unset confirmation
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y git ca-certificates sudo
if ! id "$owner" >/dev/null 2>&1; then
  useradd --create-home --shell /bin/bash "$owner"
fi
owner_home="$(getent passwd "$owner" | cut -d: -f6)"
install -d -o "$owner" -g "$(id -gn "$owner")" -m 755 "$destination"
runuser -u "$owner" -- env HOME="$owner_home" git clone --depth 1 --branch main https://github.com/Avalonbtc/local-os.git "$destination"
bash "$destination/scripts/native/install.sh" --mode "$mode" --host "$host" --listen "$listen" --no-start
export RIGDECK_ADMIN_PASSWORD="$password"
unset password
runuser -u "$owner" -- env HOME="$owner_home" bash "$destination/scripts/native/rigdeck.sh" create-admin admin
unset RIGDECK_ADMIN_PASSWORD
systemctl restart rigdeck
if [[ "$mode" == https ]]; then systemctl restart caddy; port=8080; else port=18082; fi
healthy=0
for attempt in {1..30}; do
  if curl -fsS --max-time 3 "http://127.0.0.1:$port/healthz" >/dev/null; then healthy=1; break; fi
  sleep 2
done
[[ "$healthy" == 1 ]] || die 'Health check failed; see journalctl -u rigdeck'
printf '\nInstalled in %s. Login user: admin.\n' "$destination"
case "$mode" in
  https) printf 'Open https://%s after trusting /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt and configuring hostname resolution.\n' "$host";;
  tunnel) echo 'Forward local port 18081 to server 127.0.0.1:18082 with SSH, then open http://localhost:18081';;
  cloudflared) printf 'Configure your existing Cloudflare Tunnel to forward https://%s to http://127.0.0.1:18082 (tunnel is not installed by this script).\n' "$host";;
esac
