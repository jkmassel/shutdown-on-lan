#!/bin/bash
#
# Checks what reinstalling the .pkg does to the service. Run as root after installing the package and running
# test-package.sh.
set -euo pipefail

PKG="${1:?Usage: $0 <path to shutdownonlan.pkg>}"
LABEL=com.jkmassel.shutdownonlan
PLIST="/Library/LaunchDaemons/$LABEL.plist"
PREFERENCES="/Library/Preferences/$LABEL.plist"

loaded() {
    launchctl print "system/$LABEL" > /dev/null 2>&1
}

echo "--- A reinstall leaves a disabled service disabled"
launchctl disable "system/$LABEL"
launchctl bootout "system/$LABEL"
installer -pkg "$PKG" -target /
if loaded; then
    echo "The reinstall started a service that was disabled"
    exit 1
fi
launchctl enable "system/$LABEL"
launchctl load "$PLIST"
loaded

echo "--- A reinstall that can't set up the configuration still leaves the service loaded"
BACKUP="$(mktemp)"
cp "$PREFERENCES" "$BACKUP"
echo "not a plist" > "$PREFERENCES"
if installer -pkg "$PKG" -target /; then
    echo "The installer didn't report that setting up the configuration failed"
    exit 1
fi
loaded
launchctl print-disabled system | grep -q "\"$LABEL\" => enabled"

# Restore the configuration, and make cfprefsd read it again
cp "$BACKUP" "$PREFERENCES"
rm "$BACKUP"
killall cfprefsd || true
launchctl kickstart -k "system/$LABEL"

echo "--- Reinstall test passed"
