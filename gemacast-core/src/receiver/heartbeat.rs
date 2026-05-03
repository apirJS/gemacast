use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;

pub fn spawn_heartbeat_thread(
    target: std::net::IpAddr,
    port: Arc<AtomicU16>,
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
            let p = port.load(Ordering::Relaxed);
            let target_addr = std::net::SocketAddr::new(target, p);
            let _ = socket.send_to(&[0u8], target_addr);
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    })
}
