#!/bin/sh
# Gemacast .deb postinst — best-effort firewall and port configuration.
#
# Opens the inbound-LAN ports (UDP 23555 discovery, UDP 23556 audio, TCP 23559
# control/TLS) so a phone on the same network can discover and stream from this
# PC. Mirrors the Windows MSI's firewall rules; without it, discovery silently
# fails on default-blocking firewalls (notably Fedora, where firewalld is on).
# Also reserves the port block against ephemeral allocation.
#
# Every step is guarded and the script always exits 0: a firewall we cannot
# configure must never fail the package install.
set -e

# dpkg passes "configure"; rpm (alien carries this script into %post) passes an
# install count instead - 1 on a fresh install, 2 on an upgrade. Match both, or
# the whole body silently no-ops on Fedora, the one distro that blocks by default.
case "$1" in
    configure|1|2)
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
            ufw allow 23555,23556/udp >/dev/null 2>&1 || true
            ufw allow 23559/tcp >/dev/null 2>&1 || true
        fi

        # 23555-23559 is already below the 32768 default, so this only matters on
        # a host whose ip_local_port_range was widened downward. Merge, never
        # overwrite: the sysctl is one list and another package may own entries.
        reserved=$(sysctl -n net.ipv4.ip_local_reserved_ports 2>/dev/null || true)
        case ",${reserved}," in
            *",23555-23559,"*) ;;
            *) reserved="${reserved:+${reserved},}23555-23559" ;;
        esac
        mkdir -p /etc/sysctl.d 2>/dev/null || true
        printf 'net.ipv4.ip_local_reserved_ports = %s\n' "$reserved" \
            > /etc/sysctl.d/99-gemacast.conf 2>/dev/null || true
        sysctl -p /etc/sysctl.d/99-gemacast.conf >/dev/null 2>&1 || true
        ;;
esac

exit 0
