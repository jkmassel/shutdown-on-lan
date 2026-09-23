# On uninstall (but not upgrade), stop and disable the service
if [ "$1" -eq 0 ] && [ -d /run/systemd/system ]; then
    systemctl disable --now shutdown-on-lan.service
fi
