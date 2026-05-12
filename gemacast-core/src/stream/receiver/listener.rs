use crate::{
    audio::{MAX_OPUS_PACKET_SIZE, SEQ_NUM_SIZE},
    error::{AudioError, GemaCastError, StreamDirection},
    jitter::RawPacket,
    network::Ports,
    types::JitterConfig,
};
use cpal::StreamError;
#[cfg(not(target_os = "android"))]
use cpal::traits::*;
#[cfg(target_os = "android")]
use oboe::{AudioStream, AudioStreamSafe};
use ringbuf::{HeapProd, HeapRb, traits::*};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering},
};
use tokio::sync::{mpsc, oneshot};

use super::heartbeat::spawn_keepalive_heartbeat_thread;
use super::packet::{compute_rms, parse_packet};
use super::stream::{PlaybackStream, build_playback_stream};

const PACKET_CHANNEL_CAPACITY: usize = 1024;

pub struct AudioStreamReceiver {
    packet_producer: HeapProd<RawPacket>,
    playback_stream: PlaybackStream,
    stream_error_rx: mpsc::Receiver<StreamError>,
    playback_shutdown_rx: oneshot::Receiver<()>,
    latency_metric: Arc<AtomicU32>,
}

impl AudioStreamReceiver {
    pub fn new(
        config_ref: Arc<std::sync::RwLock<JitterConfig>>,
        is_tcp_mode: Arc<AtomicBool>,
        is_playing: Arc<AtomicBool>,
        volume: Arc<AtomicU32>,
        _exclusive_mode: bool,
        playback_shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<Self, GemaCastError> {
        let (_stream_error_tx, stream_error_rx) = mpsc::channel::<StreamError>(1);
        let packet_rb = HeapRb::<RawPacket>::new(PACKET_CHANNEL_CAPACITY);
        let (packet_producer, packet_consumer) = packet_rb.split();
        let latency_metric = Arc::new(AtomicU32::new(0));

        #[cfg(not(target_os = "android"))]
        let playback_stream = build_playback_stream(
            packet_consumer,
            config_ref,
            is_tcp_mode,
            is_playing,
            volume,
            latency_metric.clone(),
            _stream_error_tx,
        )?;

        #[cfg(target_os = "android")]
        let playback_stream = build_playback_stream(
            packet_consumer,
            config_ref,
            is_tcp_mode,
            is_playing,
            volume,
            latency_metric.clone(),
            _exclusive_mode,
        )?;

        Ok(Self {
            packet_producer,
            playback_stream,
            stream_error_rx,
            playback_shutdown_rx,
            latency_metric,
        })
    }

    pub async fn run_audio_receive_loop(
        mut self,
        sender_ip_tx: Option<oneshot::Sender<String>>,
        latency_tx: Option<mpsc::Sender<(f32, f32)>>,
        target_ip: Option<std::net::IpAddr>,
        mode: crate::types::ConnectionMode,
        device_id: String,
    ) -> Result<(), GemaCastError> {
        let (transport, heartbeat_socket) = super::transport::create_audio_transport(
            mode,
            target_ip,
            &crate::types::DeviceId(device_id),
        )?;
        let heartbeat_active = Arc::new(AtomicBool::new(true));
        let sender_port = Arc::new(AtomicU16::new(Ports::AUDIO_UDP));

        let heartbeat_thread = match (target_ip, heartbeat_socket) {
            (Some(target), Some(hb_socket)) => Some(spawn_keepalive_heartbeat_thread(
                target,
                sender_port.clone(),
                heartbeat_active.clone(),
                hb_socket,
            )),
            _ => None,
        };

        let _playback_stream = self.playback_stream;
        let receiver_active = Arc::new(AtomicBool::new(true));
        let (network_dropped_tx, mut network_dropped_rx) = mpsc::channel::<()>(1);

        let receiver_thread = spawn_packet_receive_thread(
            transport,
            self.packet_producer,
            self.latency_metric.clone(),
            sender_ip_tx,
            latency_tx,
            receiver_active.clone(),
            sender_port,
            network_dropped_tx,
        );

        struct ScopeGuard {
            heartbeat_active: Arc<AtomicBool>,
            receiver_active: Arc<AtomicBool>,
            heartbeat_thread: Option<std::thread::JoinHandle<()>>,
            receiver_thread: Option<std::thread::JoinHandle<()>>,
        }

        impl Drop for ScopeGuard {
            fn drop(&mut self) {
                self.heartbeat_active.store(false, Ordering::Relaxed);
                self.receiver_active.store(false, Ordering::Relaxed);
                if let Some(t) = self.heartbeat_thread.take() {
                    let _ = t.join();
                }
                if let Some(t) = self.receiver_thread.take() {
                    let _ = t.join();
                }
            }
        }

        let mut _guard = ScopeGuard {
            heartbeat_active,
            receiver_active,
            heartbeat_thread,
            receiver_thread: Some(receiver_thread),
        };

        tokio::select! {
            Some(stream_err) = self.stream_error_rx.recv() => {
                return Err(AudioError::StreamError(stream_err).into());
            }
            _ = network_dropped_rx.recv() => {
                return Err(crate::error::NetworkError::ConnectionLost.into());
            }
            _ = &mut self.playback_shutdown_rx => {}
        }

        Ok(())
    }

    pub fn activate_playback_stream(&mut self) -> Result<(), GemaCastError> {
        #[cfg(not(target_os = "android"))]
        self.playback_stream
            .play()
            .map_err(|e| AudioError::PlayStreamFailed {
                direction: StreamDirection::Output,
                source: e,
            })?;

        #[cfg(target_os = "android")]
        {
            let burst = self.playback_stream.get_frames_per_burst();
            let _ = self.playback_stream.set_buffer_size_in_frames(burst * 2);

            self.playback_stream
                .start()
                .map_err(|_| AudioError::PlayStreamFailed {
                    direction: StreamDirection::Output,
                    source: cpal::PlayStreamError::DeviceNotAvailable,
                })?;
        }
        Ok(())
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal thread-spawn helper; struct wrapping adds no clarity"
)]
fn spawn_packet_receive_thread(
    mut transport: Box<dyn crate::stream::transport::AudioPacketTransport>,
    mut packet_producer: HeapProd<RawPacket>,
    latency_metric: Arc<AtomicU32>,
    mut sender_ip_tx: Option<oneshot::Sender<String>>,
    latency_tx: Option<mpsc::Sender<(f32, f32)>>,
    active: Arc<AtomicBool>,
    sender_port: Arc<AtomicU16>,
    network_dropped_tx: mpsc::Sender<()>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        #[cfg(target_os = "android")]
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, -19);
            libc::prctl(29, 1);
        }

        let mut recv_buff =
            vec![0u8; SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE];
        let mut last_packet_time = std::time::Instant::now();

        while active.load(Ordering::Relaxed) {
            let result = transport.receive_audio_packet(&mut recv_buff);
            let (len, sender_addr) = match result {
                Ok(r) => {
                    last_packet_time = std::time::Instant::now();
                    r
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::UnexpectedEof
                        || e.kind() == std::io::ErrorKind::ConnectionReset
                    {
                        let _ = network_dropped_tx.try_send(());
                        break;
                    }
                    if last_packet_time.elapsed().as_secs() >= 3 {
                        let _ = network_dropped_tx.try_send(());
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
            };

            sender_port.store(sender_addr.port(), Ordering::Relaxed);

            if let Some(tx) = sender_ip_tx.take() {
                let _ = tx.send(sender_addr.ip().to_string());
            }

            let Some(packet) = parse_packet(&recv_buff, len) else {
                continue;
            };

            let seq_num = packet.seq_num;
            let is_silence = packet.is_silence;
            let is_uncompressed = packet.is_uncompressed;

            if packet_producer.try_push(packet).is_err() {
                eprintln!(
                    "[WARN] SPSC ring buffer full, dropped seq {}. Audio callback may be stalled.",
                    seq_num
                );
            }

            if let Some(ref tx) = latency_tx
                && seq_num.is_multiple_of(100)
            {
                let rms_data = &recv_buff[SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE..len];
                let rms = compute_rms(rms_data, is_silence, is_uncompressed);
                let jitter_delay_ms = latency_metric.load(Ordering::Relaxed) as f32;
                let _ = tx.try_send((jitter_delay_ms, rms));
            }
        }
    })
}
