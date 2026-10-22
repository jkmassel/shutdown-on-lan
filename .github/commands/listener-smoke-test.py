#!/usr/bin/env python3
"""End-to-end checks for the listener, run against a debug build in CI.

These only ever send *wrong* secrets – the real one would shut the machine down. The machine's configuration is
saved first, and restored afterwards.

On macOS and Linux this must run as root, because the configuration lives in the system-wide preferences
domain and /etc respectively. On Windows it must run as an administrator, because the configuration lives in
HKEY_LOCAL_MACHINE.
"""

import atexit
import errno
import os
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

PORT = 53999
ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target" / "debug" / ("shutdown-on-lan.exe" if os.name == "nt" else "shutdown-on-lan")
WORKDIR = Path(tempfile.mkdtemp(prefix="shutdown-on-lan-"))
SECRET = "correct-horse-" + secrets.token_hex(16)

ENV = dict(os.environ)

LINUX_CONFIGURATION = Path("/etc/shutdown-on-lan.toml")
MACOS_PREFERENCES = "/Library/Preferences/com.jkmassel.shutdownonlan"
MACOS_SECRET = Path("/Library/Application Support/ShutdownOnLan/secret")
WINDOWS_KEY = r"HKLM\SOFTWARE\ShutdownOnLan"

logs = []


def fail(message, output=""):
    print(f"::error::{message}")
    if output:
        print(output)
    sys.exit(1)


def cli(*args, expect_success=True, input=None):
    result = subprocess.run(
        [str(BINARY), *args], cwd=WORKDIR, env=ENV, capture_output=True, text=True, input=input
    )
    output = result.stdout + result.stderr
    logs.append(output)

    if (result.returncode == 0) != expect_success:
        fail(f"`{' '.join(args)}` exited with {result.returncode}", output)

    return output


def run(*args):
    return subprocess.run(args, capture_output=True, text=True)


def save_file(path):
    """Returns a function that puts `path` back as it is now – including removing it, if it doesn't exist."""
    backup = WORKDIR / f"{path.name}.original"
    existed = path.exists()
    if existed:
        shutil.copy2(path, backup)

    def restore():
        if existed:
            shutil.copy2(backup, path)
        else:
            path.unlink(missing_ok=True)

    return restore


def save_configuration():
    """Saves the machine's configuration, and restores it when the checks finish, however they finish."""
    if sys.platform == "win32":
        backup = WORKDIR / "configuration.reg"
        existed = run("reg", "export", WINDOWS_KEY, str(backup), "/y").returncode == 0

        def restore():
            run("reg", "delete", WINDOWS_KEY, "/f")
            if existed:
                run("reg", "import", str(backup))
                # Importing doesn't restore the key's permissions, which `init` restricts again
                run(str(BINARY), "init")
    elif sys.platform == "darwin":
        backup = WORKDIR / "preferences.plist"
        # Through cfprefsd, which the file on disk can lag behind
        existed = run("defaults", "export", MACOS_PREFERENCES, str(backup)).returncode == 0
        restore_secret = save_file(MACOS_SECRET)

        def restore():
            if existed:
                run("defaults", "import", MACOS_PREFERENCES, str(backup))
            else:
                run("defaults", "delete", MACOS_PREFERENCES)
            restore_secret()
    else:
        restore = save_file(LINUX_CONFIGURATION)

    atexit.register(restore)


class Listener:
    """Runs `shutdown-on-lan run` for the duration of a `with` block."""

    def __enter__(self):
        self.log_path = WORKDIR / f"run-{len(logs)}.log"
        self.log = open(self.log_path, "w")
        self.process = subprocess.Popen(
            [str(BINARY), "run"], cwd=WORKDIR, env=ENV, stdout=self.log, stderr=subprocess.STDOUT
        )
        self.wait_for("Listening on port")
        return self

    def __exit__(self, *_):
        self.process.terminate()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
        self.log.close()
        logs.append(self.output())

    def output(self):
        return self.log_path.read_text(encoding="utf-8", errors="replace")

    def wait_for(self, text, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if text in self.output():
                return
            if self.process.poll() is not None:
                fail(f"Listener exited while waiting for {text!r}", self.output())
            time.sleep(0.1)

        fail(f"Timed out waiting for {text!r}", self.output())


def send(message, host="127.0.0.1"):
    """Sends `message`, closes the sending side, and waits for the listener to close the connection.
    Returns how long that took."""
    start = time.monotonic()

    with socket.create_connection((host, PORT), timeout=30) as connection:
        try:
            connection.sendall(message)
            connection.shutdown(socket.SHUT_WR)
            while connection.recv(1024):
                pass
        except OSError as error:
            # Expected when the listener closes a connection without reading all of it – `shutdown` fails with
            # ENOTCONN if it's already closed completely
            closed = (ConnectionResetError, ConnectionAbortedError, BrokenPipeError)
            if not isinstance(error, closed) and error.errno != errno.ENOTCONN:
                raise

    return time.monotonic() - start


def lan_address():
    """The address of the interface used for outbound traffic – no packets are sent."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
            probe.connect(("192.0.2.1", 9))
            address = probe.getsockname()[0]
    except OSError:
        return None

    return None if address.startswith("127.") else address


def check(name):
    print(f"--- {name}")


save_configuration()

check("`set` rejects invalid values")
cli("set", "--ip-address", "10.0.0.300", expect_success=False)
cli("set", "--secret", "", expect_success=False)
cli("set", "--allowed-sources", "not-an-address", expect_success=False)

check("`set` and `get` round-trip")
cli(
    "set",
    "--port", str(PORT),
    "--ip-address", "127.0.0.1",
    "--secret", SECRET,
    "--allowed-sources", "192.0.2.1",
)
output = cli("get", "--port", "--ip-addresses", "--allowed-sources")
for expected in (f"Current Port: {PORT}", "Listening IP Addresses: 127.0.0.1", "Allowed Sources: 192.0.2.1"):
    if expected not in output:
        fail(f"`get` output is missing {expected!r}", output)

check("`set --secret -` reads the secret from standard input")
cli("set", "--secret", "-", input="from-standard-input\n")
output = cli("get", "--secret")
if "Secret: from-standard-input" not in output:
    fail("`set --secret -` didn't set the secret from standard input", output)
cli("set", "--secret", SECRET)

check("Clients not in `allowed_sources` are rejected")
with Listener() as listener:
    send(b"not-the-secret\n")
    listener.wait_for("the configuration only allows connections from 192.0.2.1")

cli("set", "--allowed-sources", "")

check("Wrong secrets are read and the connection is closed")
with Listener() as listener:
    send(b"not-the-secret\n")
    listener.wait_for("didn't match the secret")
    listener.wait_for("Connection closed by 127.0.0.1")

    address = lan_address()
    if address:
        check(f"Connections on an interface that isn't configured ({address}) are rejected")
        send(b"not-the-secret\n", host=address)
        listener.wait_for(f"on {address} – the configuration only allows connections on 127.0.0.1")
    else:
        print("::warning::No LAN address found – skipping the interface check")

    check("Oversized messages are rejected")
    send(b"a" * 5000)
    listener.wait_for("message exceeds the maximum secret length")

check("Wrong secrets are throttled per source")
with Listener() as listener:
    # Attempts are allowed at 0, 100, 300, 700, 1500 and 3100ms
    elapsed = send(b"w1\nw2\nw3\nw4\nw5\nw6\n")
    print(f"6 wrong secrets on one connection took {elapsed:.2f}s (expected ~3.1s)")
    if not 2.9 <= elapsed <= 6:
        fail(f"Expected 6 wrong secrets to take ~3.1s, but they took {elapsed:.2f}s", listener.output())

    # The next slot is 3.2s after the last one – reconnecting mustn't reset it
    elapsed = send(b"w7\n")
    print(f"A wrong secret on a new connection took {elapsed:.2f}s (expected ~3.2s)")
    if not 2.9 <= elapsed <= 6.5:
        fail(f"Expected a new connection to wait ~3.2s, but it took {elapsed:.2f}s", listener.output())

check("Open connections from one source are capped")
with Listener() as listener:
    connections = [socket.create_connection(("127.0.0.1", PORT), timeout=10) for _ in range(5)]
    listener.wait_for("too many open connections from this source")
    for connection in connections:
        connection.close()

if socket.has_ipv6:
    check("IPv6 connections are accepted")
    cli("set", "--ip-address", "::1")
    with Listener() as listener:
        send(b"not-the-secret\n", host="::1")
        listener.wait_for("Connection closed by [::1]")

    check("IPv4 connections still match IPv4 addresses")
    cli("set", "--ip-address", "127.0.0.1", "--allowed-sources", "127.0.0.1")
    with Listener() as listener:
        send(b"not-the-secret\n")
        listener.wait_for("Connection closed by 127.0.0.1")
    cli("set", "--allowed-sources", "")
else:
    print("::warning::IPv6 is unavailable – skipping the IPv6 checks")

check("The secret never appears in the logs")
for log in logs:
    if SECRET in log:
        fail("The secret appears in the logs", log)

print("All listener checks passed")
