# Mirrors `%systemd_post`, except the service is enabled on first install like on every other platform.
# The default configuration only accepts connections on 127.0.0.1, so this doesn't expose anything.
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
    if [ "$1" -eq 1 ]; then
        systemctl enable --now shutdown-on-lan.service
    fi
fi
