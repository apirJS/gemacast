#!/bin/sh
# Gemacast .deb postinst — best-effort firewall configuration.
#
# Opens the inbound-LAN ports (UDP 55555 discovery, UDP 55556 audio, TCP 55559
# control/TLS) so a phone on the same network can discover and stream from this
# PC. Mirrors the Windows MSI's firewall rules; without it, discovery silently
# fails on default-blocking firewalls (notably Fedora, where firewalld is on).
#
# Every step is guarded and the script always exits 0: a firewall we cannot
# configure must never fail the package install.
set -e

case "$1" in
    configure)
        # firewalld (Fedora, RHEL, and opt-in elsewhere). The service definition
        # was installed to /usr/lib/firewalld/services/gemacast.xml; reload so a
        # running daemon picks it up, add it to the default zone, reload to apply.
        if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
            firewall-cmd --reload >/dev/null 2>&1 || true
            firewall-cmd --permanent --add-service=gemacast >/dev/null 2>&1 || true
            firewall-cmd --reload >/dev/null 2>&1 || true
        fi

        # ufw (Debian/Ubuntu). Idempotent — ufw skips a rule it already has.
        if command -v ufw >/dev/null 2>&1; then
            ufw allow 55555,55556/udp >/dev/null 2>&1 || true
            ufw allow 55559/tcp >/dev/null 2>&1 || true
        fi
        ;;
esac

exit 0
