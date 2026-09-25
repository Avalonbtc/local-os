#!/usr/bin/env bash
# Rebuild first while the running controller keeps serving; restart only if the build succeeds.
# A compile error therefore never takes the panel down.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
owner="$(stat -c %U "$repo")"
if [[ "$(id -un)" == "$owner" ]]; then
  bash "$repo/scripts/native/build.sh"
else
  sudo -u "$owner" -H bash "$repo/scripts/native/build.sh"
fi
sudo systemctl restart rigdeck
sleep 2
systemctl --no-pager --lines=0 status rigdeck || true
echo "日志：journalctl -u rigdeck -f"
