#!/bin/bash
# End-to-end checks for the macOS configuration, which lives in the system-wide preferences domain.
# Must run as root (with `sudo`, so `SUDO_USER` is set), and changes system state – only run it on CI.
set -euo pipefail

BINARY="$(cd "$(dirname "$0")/../.." && pwd)/target/debug/shutdown-on-lan"
DOMAIN="com.jkmassel.shutdownonlan"
PREFERENCES="/Library/Preferences/$DOMAIN.plist"
MANAGED_PREFERENCES="/Library/Managed Preferences/$DOMAIN.plist"
LEGACY_DIRECTORY="/Library/Application Support/ShutdownOnLan"

fail() {
    echo "::error::$1"
    [ $# -gt 1 ] && echo "$2"
    exit 1
}

check() {
    echo "--- $1"
}

# Runs the CLI as the user who invoked `sudo`
as_user() {
    sudo -u "$SUDO_USER" "$BINARY" "$@"
}

expect_private() {
    local mode
    mode=$(stat -f %Lp "$PREFERENCES")
    [ "$mode" = "600" ] || fail "$PREFERENCES has mode $mode, but should only be readable by root"
}

# Reads a stored value straight from the file. `defaults` would also work, but even `defaults read`
# rewrites the file as world-readable.
stored() {
    plutil -extract "$1" raw -o - "$PREFERENCES"
}

expect_output() {
    local output="$1" expected="$2"
    grep -qF -- "$expected" <<<"$output" || fail "Expected output to contain '$expected'" "$output"
}

reset() {
    rm -f "$PREFERENCES" "$MANAGED_PREFERENCES"
    rm -rf "$LEGACY_DIRECTORY"
    # cfprefsd caches domains, so make it re-read them from disk
    killall cfprefsd 2>/dev/null || true
}

trap reset EXIT
reset

check "A legacy configuration file is migrated, then removed"
mkdir -p "$LEGACY_DIRECTORY"
cat > "$LEGACY_DIRECTORY/ShutDownOnLan.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>port_number</key>
	<integer>54321</integer>
	<key>addresses</key>
	<array>
		<string>127.0.0.1</string>
	</array>
	<key>secret</key>
	<string>legacy-secret</string>
</dict>
</plist>
PLIST
output=$("$BINARY" get --port --ip-addresses --allowed-sources 2>&1)
expect_output "$output" "Current Port: 54321"
expect_output "$output" "Listening IP Addresses: 127.0.0.1"
expect_output "$output" "Allowed Sources: any"
[ ! -e "$LEGACY_DIRECTORY" ] || fail "$LEGACY_DIRECTORY still exists after migrating"
expect_private
[ "$(stored port_number)" = "54321" ] || fail "port_number wasn't migrated"
[ "$(stored secret)" = "legacy-secret" ] || fail "secret wasn't migrated"

check "Changes made with \`defaults\` are read, and the file is restricted again"
defaults write "$PREFERENCES" port_number -int 54322
[ "$(stat -f %Lp "$PREFERENCES")" = "644" ] || echo "::warning::\`defaults write\` no longer makes the file world-readable"
expect_output "$("$BINARY" get --port 2>&1)" "Current Port: 54322"
expect_private

check "The secret can't be read without root"
if output=$(sudo -u "$SUDO_USER" defaults read "$PREFERENCES" secret 2>&1); then
    fail "\`defaults read\` returned the secret without root" "$output"
fi
if output=$(as_user get --port 2>&1); then
    fail "\`get\` read the configuration without root" "$output"
fi

check "Changing the configuration requires root"
if output=$(as_user set --port 1 2>&1); then
    fail "\`set\` succeeded without root" "$output"
fi
expect_output "$output" "requires sudo"
[ "$(stored port_number)" = "54322" ] || fail "port_number changed without root"

check "\`set\` writes to the system-wide domain"
"$BINARY" set --port 54324 --allowed-sources 192.0.2.1 >/dev/null
expect_private
[ "$(stored port_number)" = "54324" ] || fail "\`set\` didn't write port_number"
[ "$(stored allowed_sources.0)" = "192.0.2.1" ] || fail "\`set\` didn't write allowed_sources"

check "Values managed by a configuration profile take precedence, and can't be changed"
mkdir -p "/Library/Managed Preferences"
defaults write "$MANAGED_PREFERENCES" port_number -int 54323
killall cfprefsd 2>/dev/null || true
expect_output "$("$BINARY" get --port 2>&1)" "Current Port: 54323"
if output=$("$BINARY" set --port 1 2>&1); then
    fail "\`set\` changed a managed value" "$output"
fi
expect_output "$output" "managed by a configuration profile"

check "Values that aren't managed can still be changed"
"$BINARY" set --secret "new-secret" >/dev/null
[ "$(stored secret)" = "new-secret" ] || fail "\`set\` didn't write secret"
expect_private

echo "All macOS preferences checks passed"
