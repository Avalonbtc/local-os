#!/bin/sh
# Fixed runtime dependencies only; never install package-supplied command names.
set -eu
missing=""
for dependency in screen:screen python3:python3 bash:bash unshare:util-linux mount:mount jq:jq curl:curl; do
  binary=${dependency%%:*}
  package=${dependency#*:}
  if ! command -v "$binary" >/dev/null 2>&1; then
    missing="$missing $package"
  fi
done
if [ -n "$missing" ]; then
  command -v apt-get >/dev/null 2>&1 || { echo "Missing runtime packages:$missing (apt-get unavailable)" >&2; exit 1; }
  echo "Installing missing runtime packages:$missing"
  timeout -k 10s 90s apt-get update -qq -o Acquire::Retries=0 -o Acquire::http::Timeout=20 -o Acquire::https::Timeout=20
  timeout -k 10s 120s env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends -o DPkg::Lock::Timeout=30 -o Acquire::Retries=0 -o Acquire::http::Timeout=20 -o Acquire::https::Timeout=20 $missing
fi
for binary in screen python3 bash unshare mount jq curl; do
  command -v "$binary" >/dev/null 2>&1 || { echo "Missing dependency after installation: $binary" >&2; exit 1; }
done
