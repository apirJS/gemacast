#!/bin/sh
# Gemacast .deb postrm — best-effort firewall teardown.
#
# Removes the firewall openings added by the postinst. Runs on `remove` and
# `purge`. Every step is guarded and the script always exits 0: failing to undo a
# firewall rule must never fail package removal. The service XML file itself is
# removed by dpkg (it is a packaged file), so we only drop the live rules here.
set -e

case "$1" in
    remove|purge)
        if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
            firewall-cmd --permanent --remove-service=gemacast >/dev/null 2>&1 || true
            firewall-cmd --reload >/dev/null 2>&1 || true
        fi

        if command -v ufw >/dev/null 2>&1; then
            ufw delete allow 55555,55556/udp >/dev/null 2>&1 || true
            ufw delete allow 55559/tcp >/dev/null 2>&1 || true
        fi
        ;;
esac

exit 0
