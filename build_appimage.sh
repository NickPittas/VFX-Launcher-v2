#!/usr/bin/env bash
# Build and package the merged Rust application, not the legacy Python UI.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
echo 'Building the release binary (20-minute limit), then packaging (7-minute limit).'
timeout 1200s cargo build --release --locked --manifest-path rust/Cargo.toml --bin vfx-launcher
exec timeout 420s tools/build-appimage.sh
