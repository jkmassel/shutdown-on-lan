#!/bin/bash
#
# Checks that the installed .pkg set up the service correctly. Run as root after installing the package.
# It never sends the secret, so it won't shut the machine down.
set -euo pipefail

LABEL=com.jkmassel.shutdownonlan
PREDICATE='subsystem == "com.jkmassel.shutdownonlan"'
START="$(date '+%Y-%m-%d %H:%M:%S')"

# The service logs to unified logging. `grep > /dev/null` rather than `grep -q`, which would stop reading
# early and fail `log show` with SIGPIPE under `pipefail`.
service_log_contains() {
    log show --start "$START" --predicate "$PREDICATE" --style compact | grep "$1" > /dev/null
}

service_pid() {
    launchctl print "system/$LABEL" | awk '/^\tpid = / { print $3 }'
}

wait_for_listener() {
    for _ in $(seq 20); do
        lsof -nP -iTCP:53632 -sTCP:LISTEN >/dev/null && return
        sleep 1
    done
    echo "The service isn't listening on port 53632"
    exit 1
}

echo "--- The binary is universal"
lipo /Library/Services/shutdownonlan -verify_arch arm64 x86_64

echo "--- The service is running and listening"
wait_for_listener
pid="$(service_pid)"
test -n "$pid"

echo "--- The tool is on the PATH and the configuration has a random secret"
/usr/local/bin/shutdown-on-lan get --secret | grep -Eq '^Secret: [0-9a-f]{32}$'
test "$(stat -f '%Lp %Su' '/Library/Application Support/ShutdownOnLan/secret')" = "600 root"

echo "--- A wrong secret sent over the network is rejected"
echo 'not the secret' | nc -w 5 127.0.0.1 53632 || true
for _ in $(seq 10); do
    service_log_contains "Connection closed by 127.0.0.1" && break
    sleep 1
done
service_log_contains "Connection closed by 127.0.0.1"

echo "--- info! is stored at a level that survives a reboot"
# Unified logging only keeps "Info" and "Debug" entries in memory
log show --start "$START" --style json \
    --predicate "$PREDICATE AND eventMessage BEGINSWITH \"Connection closed by\"" \
    | grep '"messageType" : "Default"' > /dev/null

echo "--- Nothing is written to the old log files"
test ! -e /var/log/shutdownonlan.log
test ! -e /var/log/shutdownonlan.error.log

echo "--- launchd restarts the service if it crashes"
kill -9 "$pid"
for _ in $(seq 30); do
    new_pid="$(service_pid)"
    if [ -n "$new_pid" ] && [ "$new_pid" != "$pid" ]; then
        break
    fi
    sleep 1
done
if [ -z "$new_pid" ] || [ "$new_pid" = "$pid" ]; then
    echo "The service wasn't restarted after it was killed"
    exit 1
fi
wait_for_listener

echo "--- Package test passed"
