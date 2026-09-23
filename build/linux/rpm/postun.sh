if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
    # On upgrade, restart the service so it runs the new version
    if [ "$1" -ge 1 ]; then
        systemctl try-restart shutdown-on-lan.service
    fi
fi
