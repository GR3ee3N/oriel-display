#!/bin/bash
# Prints the directory of Kodi's add-on dev-kit at KODI_DEVKIT_REF, downloading it once.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO"
# shellcheck source=build/versions.env
source build/versions.env
[[ $KODI_DEVKIT_REF =~ ^[0-9a-f]{40}$ ]] || { echo "KODI_DEVKIT_REF must be a full commit" >&2; exit 2; }
DEVKIT=$REPO/.cache/devkit/$KODI_DEVKIT_REF
if [[ ! -d $DEVKIT/include ]]; then
    # Only the dev-kit headers and their license are kept from Kodi's source.
    ARCHIVE=$REPO/.cache/devkit/xbmc-$KODI_DEVKIT_REF.tar.gz
    curl -fsSL --create-dirs -o "$ARCHIVE.part" "https://github.com/CoreELEC/xbmc/archive/$KODI_DEVKIT_REF.tar.gz"
    rm -rf "$DEVKIT.part"
    mkdir -p "$DEVKIT.part"
    tar -xzf "$ARCHIVE.part" -C "$DEVKIT.part" --strip-components=1 \
        "xbmc-$KODI_DEVKIT_REF/xbmc/addons/kodi-dev-kit/include" "xbmc-$KODI_DEVKIT_REF/LICENSES/GPL-2.0-or-later"
    mv "$DEVKIT.part/xbmc/addons/kodi-dev-kit/include" "$DEVKIT.part/include"
    rm -rf "$DEVKIT.part/xbmc" "$ARCHIVE.part"
    mv "$DEVKIT.part" "$DEVKIT"
fi
echo "$DEVKIT"
