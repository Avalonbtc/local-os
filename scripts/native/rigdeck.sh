#!/usr/bin/env bash
# Run a rigdeck command with the service's configuration, for example:
#   RIGDECK_ADMIN_PASSWORD='…' scripts/native/rigdeck.sh create-admin admin
#   scripts/native/rigdeck.sh migrate
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
set -a
# shellcheck disable=SC1091
. /etc/rigdeck/rigdeck.env
set +a
exec "$repo/target/release/rigdeck" "$@"
