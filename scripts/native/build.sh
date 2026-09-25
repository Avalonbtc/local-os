#!/usr/bin/env bash
# Build whatever changed. systemd runs this before every start, so after editing the source
# `sudo systemctl restart rigdeck` (or scripts/native/restart.sh) is all that is needed.
# Cargo and npm are incremental: an unchanged tree builds in a few seconds.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
if [[ "${RIGDECK_SKIP_BUILD:-0}" == 1 ]]; then
  exit 0
fi

echo "[rigdeck] building backend (cargo build --release)…"
cargo build --release --locked -p rigdeck

cd frontend
# node_modules copied from a Windows checkout carries Windows-only binaries (esbuild): reinstall.
platform="$(uname -sm)"
if [[ ! -d node_modules || package-lock.json -nt node_modules/.package-lock.json \
      || "$(cat node_modules/.rigdeck-platform 2>/dev/null)" != "$platform" ]]; then
  echo "[rigdeck] installing frontend dependencies (npm ci)…"
  npm ci --no-audit --no-fund
  printf '%s\n' "$platform" > node_modules/.rigdeck-platform
  rm -f dist/index.html
fi
stamp=dist/index.html
changed=""
if [[ -f "$stamp" ]]; then
  changed="$(find src public index.html vite.config.ts tsconfig.json package.json package-lock.json -newer "$stamp" -print -quit 2>/dev/null || true)"
fi
if [[ ! -f "$stamp" || -n "$changed" ]]; then
  echo "[rigdeck] building frontend (npm run build)…"
  npm run build
fi
echo "[rigdeck] build complete"
