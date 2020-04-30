#!/bin/bash
set -e

# For Debugging
pwd

rm -rf ./root

mkdir -p ./root/Library/Services
cp ../../target/release/shutdown-on-lan root/Library/Services/shutdownonlan

mkdir -p root/Library/LaunchDaemons
cp com.jkmassel.shutdownonlan.plist root/Library/LaunchDaemons/com.jkmassel.shutdownonlan.plist

pkgbuild --identifier "com.jkmassel.shutdownonlan" \
--root ./root \
--scripts ./scripts/ \
shutdownonlan.pkg || exit 1
    # --sign "Developer ID Installer: Douglas Richardson (4L84QT8KA9)" \

