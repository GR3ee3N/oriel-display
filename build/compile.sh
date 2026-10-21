#!/bin/bash
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO"
set -a
source build/versions.env
set +a
[[ $VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "VERSION must be x.y.z" >&2; exit 2; }
CACHE=$REPO/.cache
DEVKIT=$(build/devkit.sh)

# The committed bindings must match the pinned dev-kit.
build/bindgen.sh "$DEVKIT/include" "$CACHE/bindings.rs"
cmp -s "$CACHE/bindings.rs" crates/kodi-sys/src/bindings.rs \
    || { echo "crates/kodi-sys/src/bindings.rs is out of date; run tools/bindgen.sh" >&2; exit 1; }

TARGET=aarch64-unknown-linux-gnu
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
rustup toolchain install "$(sed -n 's/channel = "\(.*\)"/\1/p' rust-toolchain.toml)" --profile minimal --target "$TARGET"
cargo build --locked --release --target "$TARGET" -p vfs-oriel-display -p vfs-oriel-display-loader

ADDON=$CACHE/runtime/vfs.oriel.display
rm -rf "$CACHE/runtime"
mkdir -p "$ADDON/lib"
RELEASE=$CARGO_TARGET_DIR/$TARGET/release
# Kodi loads the loader; the add-on's name changes with every build (see crates/loader).
BUILD=$(sha256sum "$RELEASE/libvfs_oriel_display.so" | cut -c1-12)
install -m 755 "$RELEASE/libvfs_oriel_display_loader.so" "$ADDON/vfs.oriel.display.so"
install -m 755 "$RELEASE/libvfs_oriel_display.so" "$ADDON/lib/oriel-display-$BUILD.so"
cp -r addon/resources "$ADDON/resources"
python3 build/stage.py addon/addon.xml "$DEVKIT/include/kodi/versions.h" "$VERSION" > "$ADDON/addon.xml"
python3 build/check-library.py build/box.env "$ADDON/vfs.oriel.display.so" "$ADDON/lib/oriel-display-$BUILD.so"
python3 build/protocol.py "$CACHE/runtime/protocol"
echo "Built $ADDON against the Kodi dev-kit at $KODI_DEVKIT_REF."
