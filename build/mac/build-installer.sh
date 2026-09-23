#!/bin/bash
#
# Builds a universal (Apple Silicon and Intel) binary and packages it as build/mac/shutdownonlan.pkg.
set -euo pipefail

cd "$(dirname "$0")/../.."

TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
BINARIES=()
for target in "${TARGETS[@]}"; do
    rustup target add "$target"
    cargo build --release --target "$target"
    BINARIES+=("target/$target/release/shutdown-on-lan")
done

# `cargo pkgid` ends with `#<version>` or `@<version>`
PKGID="$(cargo pkgid)"
VERSION="${PKGID##*[#@]}"

cd build/mac
rm -rf ./root shutdownonlan.pkg

mkdir -p root/Library/Services
lipo -create -output root/Library/Services/shutdownonlan "${BINARIES[@]/#/../../}"

mkdir -p root/Library/LaunchDaemons
cp com.jkmassel.shutdownonlan.plist root/Library/LaunchDaemons/com.jkmassel.shutdownonlan.plist

pkgbuild --identifier "com.jkmassel.shutdownonlan" \
    --version "$VERSION" \
    --root ./root \
    --scripts ./scripts/ \
    shutdownonlan.pkg
