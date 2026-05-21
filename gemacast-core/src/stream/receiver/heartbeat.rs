use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

pub fn spawn_keepalive_heartbeat_thread(
    target: std::net::IpAddr,
    sender_audio_port: Arc<AtomicU16>,
    active: Arc<AtomicBool>,
    socket: UdpSocket,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        #[cfg(target_os = "android")]
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, -19);
            libc::prctl(29, 1);
        }

        while active.load(Ordering::Relaxed) {
            let p = sender_audio_port.load(Ordering::Relaxed);
            let target_addr = std::net::SocketAddr::new(target, p);
            let _ = socket.send_to(&[0u8], target_addr);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    })
}
