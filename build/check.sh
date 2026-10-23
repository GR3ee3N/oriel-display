#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
shellcheck -x -e SC2029 tools/*.sh build/*.sh
python3 -m unittest discover -s tests -t . -v
# Run unit tests without invoking Kodi or GL.
rustup toolchain install "$(sed -n 's/channel = "\(.*\)"/\1/p' rust-toolchain.toml)" --profile minimal \
    --component rustfmt --component clippy
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
