#!/bin/bash
# End-to-end checks for the macOS configuration: the settings live in the system-wide preferences domain, and
# the secret in a file only root can read. Must run as root (with `sudo`, so `SUDO_USER` is set), and changes
# system state – only run it on CI.
set -euo pipefail

BINARY="$(cd "$(dirname "$0")/../.." && pwd)/target/debug/shutdown-on-lan"
DOMAIN="com.jkmassel.shutdownonlan"
PREFERENCES="/Library/Preferences/$DOMAIN.plist"
STORAGE_DIRECTORY="/Library/Application Support/ShutdownOnLan"
LEGACY_FILE="$STORAGE_DIRECTORY/ShutDownOnLan.plist"
SECRET_FILE="$STORAGE_DIRECTORY/secret"

fail() {
    echo "::error::$1"
    if [ $# -gt 1 ]; then
        echo "$2"
    fi
    exit 1
}

check() {
    echo "--- $1"
}

# Runs the CLI as the user who invoked `sudo`
as_user() {
    sudo -u "$SUDO_USER" "$BINARY" "$@"
}

# Reads a value through `cfprefsd` – the file on disk can lag behind it
stored() {
    defaults read "$PREFERENCES" "$1"
}

expect_output() {
    local output="$1" expected="$2"
    grep -qF -- "$expected" <<<"$output" || fail "Expected output to contain '$expected'" "$output"
}

expect_secret() {
    [ "$(cat "$SECRET_FILE")" = "$1" ] || fail "Expected the secret file to contain '$1'"

    local mode
    mode=$(stat -f %Lp "$SECRET_FILE")
    [ "$mode" = "600" ] || fail "$SECRET_FILE has mode $mode, but should only be readable by root"

    if output=$(stored secret 2>&1); then
        fail "The secret is in the preferences domain, which any user can read" "$output"
    fi
}

reset() {
    rm -f "$PREFERENCES"
    rm -rf "$STORAGE_DIRECTORY"
    # cfprefsd caches domains, so make it re-read them from disk
    killall cfprefsd 2>/dev/null || true
}

trap reset EXIT
reset

check "A legacy configuration file is migrated, then removed"
mkdir -p "$STORAGE_DIRECTORY"
cat > "$LEGACY_FILE" <<'PLIST'
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
output=$("$BINARY" get --port --ip-addresses --allowed-sources --secret 2>&1)
expect_output "$output" "Current Port: 54321"
expect_output "$output" "Listening IP Addresses: 127.0.0.1"
expect_output "$output" "Allowed Sources: any"
expect_output "$output" "Secret: legacy-secret"
[ ! -e "$LEGACY_FILE" ] || fail "$LEGACY_FILE still exists after migrating"
[ "$(stored port_number)" = "54321" ] || fail "port_number wasn't migrated"
expect_secret "legacy-secret"

check "Settings changed with \`defaults\` are read"
defaults write "$PREFERENCES" port_number -int 54322
expect_output "$("$BINARY" get --port 2>&1)" "Current Port: 54322"

check "A secret written with \`defaults\` is moved to the secret file"
defaults write "$PREFERENCES" secret "written-with-defaults"
expect_output "$("$BINARY" get --secret 2>&1)" "Secret: written-with-defaults"
expect_secret "written-with-defaults"

check "The configuration can't be read or changed without root"
if output=$(as_user get --secret 2>&1); then
    fail "\`get\` read the secret without root" "$output"
fi
expect_output "$output" "sudo"
if output=$(as_user set --port 1 2>&1); then
    fail "\`set\` succeeded without root" "$output"
fi
expect_output "$output" "sudo"
[ "$(stored port_number)" = "54322" ] || fail "port_number changed without root"

check "\`set\` writes the settings to the preferences domain, and the secret to the secret file"
"$BINARY" set --port 54324 --allowed-sources 192.0.2.1 --secret "set-with-cli" >/dev/null
[ "$(stored port_number)" = "54324" ] || fail "\`set\` didn't write port_number"
expect_output "$(stored allowed_sources)" "192.0.2.1"
expect_secret "set-with-cli"

# Values managed by a configuration profile aren't covered here: macOS only honours managed preferences from
# an installed profile, and profiles can't be installed without MDM.

echo "All macOS preferences checks passed"
