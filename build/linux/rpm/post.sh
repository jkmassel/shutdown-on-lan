# Creates /etc/shutdown-on-lan.toml with a random secret, unless it already exists. The package doesn't
# ship it, because every installation needs its own secret.
shutdown-on-lan init

# Mirrors `%systemd_post`, except the service is enabled on first install like on every other platform
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
    if [ "$1" -eq 1 ]; then
        systemctl enable --now shutdown-on-lan.service
    fi
fi
