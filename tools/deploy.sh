#!/bin/sh
# Installs the built add-on on the box and has Kodi load it: an add-on Kodi
# already runs is disabled and enabled again, which closes its windows.
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
BOX="${BOX:?set BOX to your box, e.g. BOX=root@coreelec.local}"
[ "$#" -eq 0 ] || { echo "usage: tools/deploy.sh" >&2; exit 2; }
ADDONS=/storage/.kodi/addons
ID=vfs.oriel.display
cd "$REPO"
[ -f ".cache/runtime/$ID/$ID.so" ] || { echo "run tools/build.sh first" >&2; exit 1; }
# Keep installed files until their replacements have transferred.
rsync -rlt --delete-after --delay-updates ".cache/runtime/$ID/" "$BOX:$ADDONS/$ID/"
ssh "$BOX" "kodi-send --action=UpdateLocalAddons >/dev/null"
ssh "$BOX" python3 - "$ID" --reload < tools/enable.py
echo "installed $ID"
