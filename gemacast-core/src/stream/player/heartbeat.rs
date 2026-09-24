use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

/// Owns the phone's UDP keepalive lifecycle.
pub(crate) struct KeepaliveHeartbeat {
    target: std::net::IpAddr,
    streamer_audio_port: Arc<AtomicU16>,
    active: Arc<AtomicBool>,
    socket: UdpSocket,
}

impl KeepaliveHeartbeat {
    pub(crate) fn new(
        target: std::net::IpAddr,
        streamer_audio_port: Arc<AtomicU16>,
        active: Arc<AtomicBool>,
        socket: UdpSocket,
    ) -> Self {
        Self {
            target,
            streamer_audio_port,
            active,
            socket,
        }
    }

    /// Spawns the phone's 500 ms keepalive thread.
    ///
    /// The packet doubles as a UDP echo ping: it carries a timestamp the PC
    /// bounces back verbatim, letting the receive loop measure raw wire RTT (see
    /// [`crate::stream::echo`]). Its keepalive role is unchanged — a 10-byte ping
    /// keeps the NAT mapping and socket alive exactly as the old 1-byte packet did.
    pub(crate) fn spawn(self) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            #[cfg(target_os = "android")]
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, 0, -19);
                libc::prctl(29, 1);
            }

            while self.active.load(Ordering::Relaxed) {
                let port = self.streamer_audio_port.load(Ordering::Relaxed);
                let target_addr = std::net::SocketAddr::new(self.target, port);
                let _ = self
                    .socket
                    .send_to(&crate::stream::echo::EchoPacket::build(), target_addr);
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })
    }
}
