#!/bin/bash
#
# Installs the tools build/linux/build-packages.sh needs, at pinned versions. On x86_64, cargo-deb is installed
# from its official release package, checked against a pinned hash, rather than compiled from source, which
# takes about four minutes. There's no release package for other architectures, or for cargo-generate-rpm.
set -euo pipefail

CARGO_DEB_VERSION=3.8.0
CARGO_DEB_SHA256=c6bd0a4affaa232814cc615b2597457f1d44fdfe1c8ebd643f3a491a1f244040
CARGO_GENERATE_RPM_VERSION=0.21.0

if [ "$(uname -m)" = x86_64 ]; then
    package="$(mktemp -d)/cargo-deb.deb"
    curl -fsSL -o "$package" \
        "https://github.com/kornelski/cargo-deb/releases/download/v$CARGO_DEB_VERSION/cargo-deb_$CARGO_DEB_VERSION-1_amd64.deb"
    echo "$CARGO_DEB_SHA256  $package" | sha256sum --check --quiet
    sudo apt-get install -y "$package"
else
    cargo install --locked "cargo-deb@$CARGO_DEB_VERSION"
fi

cargo install --locked "cargo-generate-rpm@$CARGO_GENERATE_RPM_VERSION"
