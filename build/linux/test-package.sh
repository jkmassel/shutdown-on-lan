#!/bin/bash
#
# Checks that an installed .deb or .rpm set up the service correctly. Run as root, on a machine with
# systemd, after installing the package. It never sends the secret, so it won't shut the machine down.
set -euo pipefail

UNIT=/usr/lib/systemd/system/shutdown-on-lan.service

echo "--- The service is enabled and running"
systemctl is-enabled shutdown-on-lan
systemctl is-active shutdown-on-lan

echo "--- It's listening on every interface"
for _ in $(seq 10); do
    ss -ltn | grep -q ':53632 ' && break
    sleep 1
done
ss -ltn | grep ':53632 '

echo "--- The configuration has a random secret that only root can read"
test "$(stat -c '%a %U' /etc/shutdown-on-lan.toml)" = "600 root"
shutdown-on-lan get --secret | grep -Eq '^Secret: [0-9a-f]{32}$'

echo "--- Other users are told they need root"
if output="$(runuser -u nobody -- shutdown-on-lan get --port 2>&1)"; then
    echo "Expected \`get\` to fail for another user, but it printed: $output"
    exit 1
fi
echo "$output" | grep -q 'try again with sudo'

echo "--- Shutting down works from inside the service's sandbox"
# `shutdown` asks systemd to power off, so check that the sandbox can still reach it. Only the unit's
# own settings are used, not any drop-ins.
properties=()
while IFS= read -r line; do
    properties+=(-p "$line")
done < <(grep -E '^(Capability|NoNewPrivileges|Protect|Private|Restrict|Lock|Memory|SystemCall)' "$UNIT")
systemd-run --wait --pipe --collect "${properties[@]}" systemctl show --property=Version

echo "--- A wrong secret sent over the network is rejected"
address="$(hostname -I | awk '{print $1}')"
echo 'not the secret' | timeout 5 nc -q1 "$address" 53632 || true
journalctl -u shutdown-on-lan --no-pager | grep "New connection"
systemctl is-active shutdown-on-lan

echo "--- Package test passed"
