//! Best-effort Linux firewall diagnostics.
//!
//! The deb/rpm packages open the LAN ports from their maintainer scripts
//! ([linux/deb-postinst.sh](../../linux/deb-postinst.sh)), but the portable
//! **AppImage runs no privileged install step**, so on a default-blocking
//! firewall an AppImage user's phone silently fails to discover or stream and
//! there is nothing on disk to tell them why. This module fills that gap: at
//! startup, on Linux, it inspects the live firewall and — only if it looks like
//! our ports are actually blocked — hands the caller a message naming the exact
//! unblock command, which the tray shows in a dialog.
//!
//! It must not log the message instead. `gemacast-pc` builds with
//! `release_max_level_off` ([Cargo.toml](../Cargo.toml)), so every `tracing::*`
//! call in this crate is compiled out of a release binary — a warning logged here
//! reaches nobody in the builds users actually download.
//!
//! Discovery + streaming need three **inbound** LAN openings: UDP
//! [`Ports::DISCOVERY`], UDP [`Ports::AUDIO_UDP`], TCP [`Ports::CONTROL`]. The
//! ADB/USB transport is loopback (`adb reverse`) and needs none of them, which
//! is why the hint always offers USB as the no-firewall escape hatch.
//!
//! The decision logic is a set of pure functions unit-tested on every platform;
//! only the process-shelling glue is Linux-gated. On uncertainty (daemon
//! present but config unreadable) we deliberately stay **silent** rather than
//! risk a false positive — notably on Fedora Workstation, whose default zone
//! opens `1025-65535` for both protocols, so our ports are already reachable
//! there and must not trigger a warning.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use gemacast_core::network::Ports;

/// Transport protocol of a required port.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Proto {
    Udp,
    Tcp,
}

impl Proto {
    fn as_str(self) -> &'static str {
        match self {
            Proto::Udp => "udp",
            Proto::Tcp => "tcp",
        }
    }
}

/// The inbound-LAN openings discovery + streaming require. ADB (loopback) needs
/// none of these.
const REQUIRED_PORTS: [(u16, Proto); 3] = [
    (Ports::DISCOVERY, Proto::Udp),
    (Ports::AUDIO_UDP, Proto::Udp),
    (Ports::CONTROL, Proto::Tcp),
];

/// Whether firewalld's `--list-ports` output covers `port`/`proto`, honoring
/// range entries (e.g. Fedora Workstation's `1025-65535/tcp`) as well as exact
/// single-port entries. Tokens are `PORT/proto` or `START-END/proto`.
fn firewalld_ports_cover(list_ports: &str, port: u16, proto: Proto) -> bool {
    list_ports.split_whitespace().any(|tok| {
        let Some((range, tok_proto)) = tok.split_once('/') else {
            return false;
        };
        if tok_proto != proto.as_str() {
            return false;
        }
        match range.split_once('-') {
            Some((start, end)) => match (start.parse::<u16>(), end.parse::<u16>()) {
                (Ok(start), Ok(end)) => (start..=end).contains(&port),
                _ => false,
            },
            None => range.parse::<u16>() == Ok(port),
        }
    })
}

/// Whether an active firewalld blocks us, given its active/default zone's
/// `--list-services` and `--list-ports` output. Our packaged `gemacast` service
/// opens exactly the ports we need, so its presence is an immediate pass;
/// otherwise every required port must be individually (or range-) covered.
fn firewalld_blocks(list_services: &str, list_ports: &str) -> bool {
    if list_services.split_whitespace().any(|s| s == "gemacast") {
        return false;
    }
    REQUIRED_PORTS
        .iter()
        .any(|&(port, proto)| !firewalld_ports_cover(list_ports, port, proto))
}

/// Whether an enabled ufw blocks us, given `ufw status` output. ufw defaults to
/// deny-incoming when active, so we are blocked unless every required port
/// appears in an allow rule. Ranges are not parsed (rare on desktops); a bare
/// port substring match is sufficient because ufw prints the port number
/// verbatim (e.g. `23555,23556/udp  ALLOW  Anywhere`).
fn ufw_blocks(status: &str) -> bool {
    // "Status: inactive" does not contain "Status: active" as a substring.
    if !status.contains("Status: active") {
        return false;
    }
    REQUIRED_PORTS
        .iter()
        .any(|&(port, _)| !status.contains(&port.to_string()))
}

/// Decide whether to warn, and with what text.
///
/// - `firewalld`: `Some((list_services, list_ports))` if firewalld is running
///   and its config was readable, else `None` (not running / unreadable →
///   treated as non-blocking).
/// - `ufw`: `Some(status_output)` if `ufw status` was readable, else `None`.
///
/// Returns `Some(message)` only when a firewall looks like it is actively
/// blocking us; the message names the fix for whichever tool is blocking.
fn firewall_advice(firewalld: Option<(&str, &str)>, ufw: Option<&str>) -> Option<String> {
    let firewalld_blocking = firewalld.is_some_and(|(svc, ports)| firewalld_blocks(svc, ports));
    let ufw_blocking = ufw.is_some_and(ufw_blocks);

    if !firewalld_blocking && !ufw_blocking {
        return None;
    }

    let disc = Ports::DISCOVERY;
    let audio = Ports::AUDIO_UDP;
    let ctrl = Ports::CONTROL;

    // Written for a dialog, not a log line: short lead, blank lines between
    // blocks, each command on its own line so it can be selected and copied.
    let mut msg = String::from(
        "This PC's firewall looks like it is blocking Gemacast. Your phone may not find \
         this PC, or may connect but play no audio.\n\nOpen the ports by running:",
    );
    if firewalld_blocking {
        msg.push_str(&format!(
            "\n\nfirewalld:\nsudo firewall-cmd --permanent --add-port={disc}/udp \
             --add-port={audio}/udp --add-port={ctrl}/tcp && sudo firewall-cmd --reload"
        ));
    }
    if ufw_blocking {
        msg.push_str(&format!(
            "\n\nufw:\nsudo ufw allow {disc},{audio}/udp && sudo ufw allow {ctrl}/tcp"
        ));
    }
    msg.push_str("\n\nOr connect over USB instead, which needs no firewall change.");

    Some(msg)
}

/// Inspect the live Linux firewall and, if it looks like our LAN ports are
/// blocked, return a message naming the exact unblock command.
///
/// Never fails and never blocks meaningfully: each probe is a short-lived
/// read-only command, and any error (missing binary, permission denied, daemon
/// down) is treated as "cannot tell → stay silent". Called once, at engine
/// startup; the caller decides how to show the result.
#[cfg(target_os = "linux")]
pub fn firewall_warning() -> Option<String> {
    let firewalld = query_firewalld();
    let ufw = query_ufw();
    firewall_advice(
        firewalld.as_ref().map(|(s, p)| (s.as_str(), p.as_str())),
        ufw.as_deref(),
    )
}

/// Run a read-only command, returning its stdout only on a clean (exit-0) run.
/// A missing binary, non-zero exit, or spawn failure all yield `None`.
#[cfg(target_os = "linux")]
fn run_ok(program: &str, args: &[&str]) -> Option<String> {
    let output = gemacast_core::process::quiet_command(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Query firewalld's active/default-zone config. `None` = not running or config
/// unreadable, which the caller treats as non-blocking — so we never cry wolf on
/// a box we could not actually inspect (e.g. Fedora Workstation's open zone).
#[cfg(target_os = "linux")]
fn query_firewalld() -> Option<(String, String)> {
    // `--state` prints "running" and exits 0 only when the daemon is active.
    run_ok("firewall-cmd", &["--state"])?;
    // Read-only list ops; usually permitted for non-root. If they fail, bail to
    // silence rather than assume the worst.
    let services = run_ok("firewall-cmd", &["--list-services"])?;
    let ports = run_ok("firewall-cmd", &["--list-ports"])?;
    Some((services, ports))
}

/// Query ufw status. `ufw status` requires root; as a normal-user tray app we
/// usually cannot read it, so ufw detection is best-effort and typically inert
/// (returns `None` → non-blocking). The firewalld path above is the load-bearing
/// one on the default-blocking distros (Fedora/RHEL).
#[cfg(target_os = "linux")]
fn query_ufw() -> Option<String> {
    run_ok("ufw", &["status"])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Concrete port numbers, sourced from the single allocation point so the
    // tests move with the constants.
    fn disc() -> String {
        Ports::DISCOVERY.to_string()
    }
    fn audio() -> String {
        Ports::AUDIO_UDP.to_string()
    }
    fn ctrl() -> String {
        Ports::CONTROL.to_string()
    }

    mod firewalld_ports_cover {
        use super::*;

        #[test]
        fn matches_an_exact_single_port_entry() {
            let ports = format!("{}/tcp", Ports::CONTROL);
            assert!(firewalld_ports_cover(&ports, Ports::CONTROL, Proto::Tcp));
        }

        #[test]
        fn matches_a_port_inside_a_range_entry() {
            // Fedora Workstation's default zone opens 1025-65535 for both protos.
            let ports = "1025-65535/udp 1025-65535/tcp";
            assert!(firewalld_ports_cover(ports, Ports::CONTROL, Proto::Tcp));
            assert!(firewalld_ports_cover(ports, Ports::DISCOVERY, Proto::Udp));
        }

        #[test]
        fn rejects_a_matching_port_on_the_wrong_protocol() {
            let ports = format!("{}/tcp", Ports::DISCOVERY);
            assert!(!firewalld_ports_cover(&ports, Ports::DISCOVERY, Proto::Udp));
        }

        #[test]
        fn rejects_a_port_outside_every_range() {
            let ports = "1-1024/tcp 22/tcp";
            assert!(!firewalld_ports_cover(ports, Ports::CONTROL, Proto::Tcp));
        }

        #[test]
        fn ignores_malformed_tokens() {
            assert!(!firewalld_ports_cover(
                "garbage no-slash /tcp abc-def/tcp",
                Ports::CONTROL,
                Proto::Tcp
            ));
        }
    }

    mod firewalld_blocks {
        use super::*;

        #[test]
        fn passes_when_our_service_is_present() {
            assert!(!firewalld_blocks("ssh gemacast dhcpv6-client", ""));
        }

        #[test]
        fn passes_on_a_fedora_workstation_style_open_zone() {
            assert!(!firewalld_blocks(
                "dhcpv6-client ssh samba-client",
                "1025-65535/udp 1025-65535/tcp"
            ));
        }

        #[test]
        fn passes_when_every_required_port_is_listed_individually() {
            let ports = format!(
                "{}/udp {}/udp {}/tcp",
                Ports::DISCOVERY,
                Ports::AUDIO_UDP,
                Ports::CONTROL
            );
            assert!(!firewalld_blocks("ssh", &ports));
        }

        #[test]
        fn blocks_on_a_restrictive_zone_with_none_of_our_ports() {
            assert!(firewalld_blocks("dhcpv6-client ssh", ""));
        }

        #[test]
        fn blocks_when_only_some_required_ports_are_open() {
            // Control TCP present, but the two UDP discovery/audio ports are not.
            let ports = format!("{}/tcp", Ports::CONTROL);
            assert!(firewalld_blocks("ssh", &ports));
        }
    }

    mod ufw_blocks {
        use super::*;

        #[test]
        fn passes_when_inactive() {
            assert!(!ufw_blocks("Status: inactive"));
        }

        #[test]
        fn passes_when_active_and_all_ports_allowed() {
            let status = format!(
                "Status: active\n\nTo                         Action      From\n\
                 --                         ------      ----\n\
                 {},{}/udp                  ALLOW       Anywhere\n\
                 {}/tcp                     ALLOW       Anywhere\n",
                Ports::DISCOVERY,
                Ports::AUDIO_UDP,
                Ports::CONTROL
            );
            assert!(!ufw_blocks(&status));
        }

        #[test]
        fn blocks_when_active_with_no_matching_rule() {
            assert!(ufw_blocks("Status: active\n\n22/tcp ALLOW Anywhere\n"));
        }

        #[test]
        fn blocks_when_active_and_only_some_ports_allowed() {
            let status = format!("Status: active\n\n{}/tcp ALLOW Anywhere\n", Ports::CONTROL);
            assert!(ufw_blocks(&status));
        }
    }

    mod firewall_advice {
        use super::*;

        #[test]
        fn stays_silent_when_neither_firewall_blocks() {
            assert!(firewall_advice(None, None).is_none());
            assert!(firewall_advice(Some(("gemacast", "")), Some("Status: inactive")).is_none());
        }

        #[test]
        fn stays_silent_on_a_fedora_workstation_open_zone() {
            assert!(
                firewall_advice(Some(("ssh", "1025-65535/udp 1025-65535/tcp")), None).is_none()
            );
        }

        #[test]
        fn warns_with_the_firewall_cmd_fix_when_firewalld_blocks() {
            let msg = firewall_advice(Some(("ssh", "")), None).expect("should warn");
            assert!(msg.contains("firewall-cmd --permanent"));
            assert!(msg.contains(&disc()));
            assert!(msg.contains(&audio()));
            assert!(msg.contains(&ctrl()));
            // Always offer the loopback escape hatch.
            assert!(msg.contains("USB"));
        }

        #[test]
        fn warns_with_the_ufw_fix_when_ufw_blocks() {
            let msg =
                firewall_advice(None, Some("Status: active\n\n22/tcp ALLOW Anywhere\n")).unwrap();
            assert!(msg.contains("ufw allow"));
            assert!(!msg.contains("firewall-cmd"));
        }

        #[test]
        fn names_both_fixes_when_both_block() {
            let msg = firewall_advice(Some(("ssh", "")), Some("Status: active\n\n22/tcp ALLOW\n"))
                .unwrap();
            assert!(msg.contains("firewall-cmd"));
            assert!(msg.contains("ufw allow"));
        }

        #[test]
        fn formats_the_message_for_a_dialog_not_a_log_line() {
            let msg = firewall_advice(Some(("ssh", "")), Some("Status: active\n\n22/tcp ALLOW\n"))
                .unwrap();
            // The old log-shaped message indented every fix by two spaces, which a
            // dialog renders as ragged text rather than as a list.
            assert!(!msg.lines().any(|line| line.starts_with(' ')));
            // Each command alone on its line, so it can be selected and copied.
            assert!(
                msg.lines()
                    .any(|line| line.starts_with("sudo firewall-cmd"))
            );
            assert!(msg.lines().any(|line| line.starts_with("sudo ufw allow")));
        }
    }
}
