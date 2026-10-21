#!/bin/sh
# Regenerates crates/kodi-sys/src/bindings.rs after KODI_DEVKIT_REF or the binding list changes.
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
[ "$#" -eq 0 ] || { echo "usage: tools/bindgen.sh" >&2; exit 2; }
# Expanded in the container, not here.
# shellcheck disable=SC2016
exec "$REPO/build/container.sh" bash -c 'build/bindgen.sh "$(build/devkit.sh)/include" crates/kodi-sys/src/bindings.rs'
