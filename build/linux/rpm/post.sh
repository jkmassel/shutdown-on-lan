# The configuration isn't created here – the service creates it when it first starts, so that every machine
# cloned from an image the package was installed in gets its own secret.

# Mirrors `%systemd_post`, except the service is enabled on first install like on every other platform
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
fi
if [ "$1" -eq 1 ] && command -v systemctl > /dev/null; then
    # Enabling only creates symlinks, so it works without systemd running too – for instance, when the package
    # is installed while building an image
    systemctl enable shutdown-on-lan.service
    if [ -d /run/systemd/system ]; then
        systemctl start shutdown-on-lan.service
    fi
fi
