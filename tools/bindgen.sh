#!/bin/sh
# Regenerate crates/kodi-sys/src/bindings.rs when the dev-kit pin or selected headers change.
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
[ "$#" -eq 0 ] || { echo "usage: tools/bindgen.sh" >&2; exit 2; }
# Defer variable expansion to the container shell.
# shellcheck disable=SC2016
exec "$REPO/build/container.sh" bash -c 'build/bindgen.sh "$(build/devkit.sh)/include" crates/kodi-sys/src/bindings.rs'
