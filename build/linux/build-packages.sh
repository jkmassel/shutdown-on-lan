#!/bin/bash
#
# Builds a static binary for a Linux target, then packages it as a tarball, a .deb and an .rpm in
# build/linux/dist. Requires `cargo-deb` and `cargo-generate-rpm`.
#
# Usage: build/linux/build-packages.sh <x86_64|aarch64>
#
# Set CARGO_BUILD_SUBCOMMAND=zigbuild to cross-compile with cargo-zigbuild (for instance, on a Mac).
set -euo pipefail

ARCH="${1:?Usage: $0 <x86_64|aarch64>}"
TARGET="$ARCH-unknown-linux-musl"

cd "$(dirname "$0")/../.."

DIST=build/linux/dist
rm -rf "$DIST"
mkdir -p "$DIST"

# Link against musl so that one binary runs on any distribution. Strip it here, because cargo-deb would
# otherwise try to strip it with the host's `strip`, which can't handle a cross-compiled binary.
rustup target add "$TARGET"
CARGO_PROFILE_RELEASE_STRIP=true cargo "${CARGO_BUILD_SUBCOMMAND:-build}" --release --target "$TARGET"

BINARY="target/$TARGET/release/shutdown-on-lan"

cargo deb --target "$TARGET" --no-build --no-strip --output "$DIST/shutdown-on-lan-linux-$ARCH.deb"
cargo generate-rpm --target "$TARGET" --output "$DIST/shutdown-on-lan-linux-$ARCH.rpm"

# For other distributions – see "Linux" in README.md
STAGING="$(mktemp -d)"
mkdir "$STAGING/shutdown-on-lan"
cp "$BINARY" \
    build/linux/shutdown-on-lan.service \
    build/linux/firewalld/shutdown-on-lan.xml \
    README.md \
    LICENSE \
    "$STAGING/shutdown-on-lan/"
cp build/linux/ufw/shutdown-on-lan "$STAGING/shutdown-on-lan/shutdown-on-lan.ufw"
tar -czf "$DIST/shutdown-on-lan-linux-$ARCH.tar.gz" -C "$STAGING" shutdown-on-lan
rm -rf "$STAGING"

ls -l "$DIST"
