#!/bin/sh
# Gemacast .deb postrm — best-effort firewall and port teardown.
#
# Undoes the postinst. Runs on `remove` and `purge`. Every step is guarded and the
# script always exits 0: failing to undo a firewall rule must never fail package
# removal. The service XML file itself is removed by dpkg (it is a packaged file),
# so we only drop the live rules here.
set -e

# dpkg passes "remove"/"purge"; rpm (via alien's %postun) passes 0 on the final
# erase and 1 during an upgrade. Match the erase forms only.
case "$1" in
    remove|purge|0)
        if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
            firewall-cmd --permanent --remove-service=gemacast >/dev/null 2>&1 || true
            firewall-cmd --reload >/dev/null 2>&1 || true
        fi

        if command -v ufw >/dev/null 2>&1; then
            ufw delete allow 23555,23556/udp >/dev/null 2>&1 || true
            ufw delete allow 23559/tcp >/dev/null 2>&1 || true
        fi

        # Drop only our range from the running list, so any entry another package
        # put there survives. Written straight to /proc because `sysctl -w key=`
        # rejects an empty value, which is the common case.
        rm -f /etc/sysctl.d/99-gemacast.conf
        reserved=$(sysctl -n net.ipv4.ip_local_reserved_ports 2>/dev/null || true)
        reserved=$(printf '%s' "$reserved" | tr ',' '\n' | grep -vx '23555-23559' | tr '\n' ',')
        printf '%s\n' "${reserved%,}" \
            > /proc/sys/net/ipv4/ip_local_reserved_ports 2>/dev/null || true
        ;;
esac

exit 0
