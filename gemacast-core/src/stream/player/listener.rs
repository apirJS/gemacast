#[cfg(target_os = "android")]
use super::stream::build_cpal_fallback_stream;
use crate::{
    audio::{MAX_OPUS_PACKET_SIZE, SEQ_NUM_SIZE},
    domain::error::{AudioError, GemaCastError, StreamDirection},
    domain::types::{JitterConfig, NetworkLink},
    jitter::RawPacket,
    network::Ports,
};
use cpal::StreamError;
#[cfg(not(target_os = "android"))]
use cpal::traits::*;
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

#[derive(Debug, Clone)]
pub struct AudioSessionCredentials {
    pub device_id: crate::domain::types::DeviceId,
    pub session_token: Option<String>,
    pub session_generation: Option<crate::control::SessionGeneration>,
}

pub struct AudioStreamPlayer {
    packet_producer: HeapProd<RawPacket>,
    playback_stream: PlaybackStream,
    stream_error_rx: mpsc::Receiver<StreamError>,
    playback_shutdown_rx: oneshot::Receiver<()>,
    latency_metric: Arc<AtomicU32>,
    jitter_metric: Arc<AtomicU32>,
    pub exclusive_granted: bool,
}

impl AudioStreamPlayer {
    pub fn new(
        config_ref: Arc<std::sync::RwLock<JitterConfig>>,
        is_tcp_mode: Arc<AtomicBool>,
        network_link: NetworkLink,
        is_playing: Arc<AtomicBool>,
        volume: Arc<AtomicU32>,
        _exclusive_mode: bool,
        playback_shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<Self, GemaCastError> {
        let (_stream_error_tx, stream_error_rx) = mpsc::channel::<StreamError>(1);
        let packet_rb = HeapRb::<RawPacket>::new(PACKET_CHANNEL_CAPACITY);
        let (packet_producer, packet_consumer) = packet_rb.split();
        let latency_metric = Arc::new(AtomicU32::new(0));
        let jitter_metric = Arc::new(AtomicU32::new(0));

        #[cfg(not(target_os = "android"))]
        let playback_stream = build_playback_stream(
            packet_consumer,
            config_ref,
            is_tcp_mode,
            network_link,
            is_playing,
            volume,
            latency_metric.clone(),
            jitter_metric.clone(),
            _stream_error_tx,
        )?;

        #[cfg(not(target_os = "android"))]
        let exclusive_granted = false;

        #[cfg(target_os = "android")]
        let (packet_producer, playback_stream, exclusive_granted) = {
            // Try Oboe first; if it fails, the consumer is consumed by the
            // failed callback so we must create a fresh ring buffer for cpal.
            match build_playback_stream(
                packet_consumer,
                config_ref.clone(),
                is_tcp_mode.clone(),
                network_link,
                is_playing.clone(),
                volume.clone(),
                latency_metric.clone(),
                jitter_metric.clone(),
                _exclusive_mode,
            ) {
                Ok((stream, granted)) => (packet_producer, stream, granted),
                Err(oboe_err) => {
                    tracing::warn!("Oboe failed ({}), retrying with cpal fallback", oboe_err);
                    let fallback_rb = HeapRb::<RawPacket>::new(PACKET_CHANNEL_CAPACITY);
                    let (fb_producer, fb_consumer) = fallback_rb.split();
                    let stream = build_cpal_fallback_stream(
                        fb_consumer,
                        config_ref,
                        is_tcp_mode,
                        network_link,
                        is_playing,
                        volume,
                        latency_metric.clone(),
                        jitter_metric.clone(),
                    )?;
                    (fb_producer, stream, false)
                }
            }
        };

        Ok(Self {
            packet_producer,
            playback_stream,
            stream_error_rx,
            playback_shutdown_rx,
            latency_metric,
            jitter_metric,
            exclusive_granted,
        })
    }

    pub async fn run_audio_receive_loop(
        mut self,
        streamer_ip_tx: Option<oneshot::Sender<String>>,
        latency_tx: Option<mpsc::Sender<(f32, f32, f32)>>,
        rtt_tx: Option<mpsc::Sender<f32>>,
        target_ip: Option<std::net::IpAddr>,
        mode: crate::domain::types::ConnectionMode,
        credentials: AudioSessionCredentials,
    ) -> Result<(), GemaCastError> {
        let (transport, heartbeat_socket) = super::transport::create_audio_transport(
            mode,
            target_ip,
            &credentials.device_id,
            credentials.session_token.as_deref(),
            credentials.session_generation,
        )?;
        let heartbeat_active = Arc::new(AtomicBool::new(true));
        let streamer_port = Arc::new(AtomicU16::new(Ports::AUDIO_UDP));

        let heartbeat_thread = match (target_ip, heartbeat_socket) {
            (Some(target), Some(hb_socket)) => Some(spawn_keepalive_heartbeat_thread(
                target,
                streamer_port.clone(),
                heartbeat_active.clone(),
                hb_socket,
            )),
            _ => None,
        };

        let _playback_stream = self.playback_stream;
        let player_active = Arc::new(AtomicBool::new(true));
        let (network_dropped_tx, mut network_dropped_rx) = mpsc::channel::<()>(1);

        let player_thread = spawn_packet_receive_thread(
            transport,
            self.packet_producer,
            self.latency_metric.clone(),
            self.jitter_metric.clone(),
            streamer_ip_tx,
            latency_tx,
            rtt_tx,
            player_active.clone(),
            streamer_port,
            network_dropped_tx,
            target_ip,
        );

        struct ScopeGuard {
            heartbeat_active: Arc<AtomicBool>,
            player_active: Arc<AtomicBool>,
            heartbeat_thread: Option<std::thread::JoinHandle<()>>,
            player_thread: Option<std::thread::JoinHandle<()>>,
        }

        impl Drop for ScopeGuard {
            fn drop(&mut self) {
                self.heartbeat_active.store(false, Ordering::Relaxed);
                self.player_active.store(false, Ordering::Relaxed);
                if let Some(t) = self.heartbeat_thread.take() {
                    // Detach thread instead of blocking the Tokio worker
                    drop(t);
                }
                if let Some(t) = self.player_thread.take() {
                    // Detach thread instead of blocking the Tokio worker
                    drop(t);
                }
            }
        }

        let mut _guard = ScopeGuard {
            heartbeat_active,
            player_active,
            heartbeat_thread,
            player_thread: Some(player_thread),
        };

        tokio::select! {
            Some(stream_err) = self.stream_error_rx.recv() => {
                return Err(AudioError::StreamError(stream_err).into());
            }
            _ = network_dropped_rx.recv() => {
                return Err(crate::domain::error::NetworkError::ConnectionLost.into());
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
            use oboe::{AudioStream, AudioStreamSafe};
            match &mut self.playback_stream {
                PlaybackStream::Oboe(stream) => {
                    let burst = stream.get_frames_per_burst();
                    let _ = stream.set_buffer_size_in_frames(burst * 2);

                    stream
                        .start()
                        .map_err(|e| AudioError::OboeStreamStartFailed {
                            direction: StreamDirection::Output,
                            message: format!("{}", e),
                        })?;
                }
                PlaybackStream::Cpal(stream) => {
                    use cpal::traits::StreamTrait;
                    stream.play().map_err(|e| AudioError::PlayStreamFailed {
                        direction: StreamDirection::Output,
                        source: e,
                    })?;
                }
            }
        }
        Ok(())
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal thread-spawn helper; struct wrapping adds no clarity"
)]
fn spawn_packet_receive_thread<T: crate::ports::transport::AudioPacketTransport + 'static>(
    mut transport: T,
    mut packet_producer: HeapProd<RawPacket>,
    latency_metric: Arc<AtomicU32>,
    jitter_metric: Arc<AtomicU32>,
    mut streamer_ip_tx: Option<oneshot::Sender<String>>,
    latency_tx: Option<mpsc::Sender<(f32, f32, f32)>>,
    rtt_tx: Option<mpsc::Sender<f32>>,
    active: Arc<AtomicBool>,
    streamer_port: Arc<AtomicU16>,
    network_dropped_tx: mpsc::Sender<()>,
    allowed_streamer_ip: Option<std::net::IpAddr>,
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
        let mut first_packet_received = false;

        while active.load(Ordering::Relaxed) {
            let result = transport.receive_audio_packet(&mut recv_buff);
            let (len, streamer_addr) = match result {
                Ok(r) => {
                    if allowed_streamer_ip.is_some_and(|allowed| allowed != r.1.ip()) {
                        tracing::debug!(
                            allowed = %allowed_streamer_ip.unwrap(),
                            observed = %r.1.ip(),
                            "[Player] Ignoring audio packet from an unexpected streamer"
                        );
                        continue;
                    }
                    // An echo ping the PC reflected back: record the wire RTT and
                    // skip the audio path entirely. This must run before the
                    // bookkeeping below — an echo is liveness, not audio, so it
                    // must not seed the streamer IP, reset the audio-arrival
                    // timeout, or trip the "first packet" log.
                    if crate::stream::echo::is_echo(&recv_buff, r.0) {
                        if let Some(ref tx) = rtt_tx {
                            let _ = tx.try_send(crate::stream::echo::read_rtt_ms(&recv_buff));
                        }
                        continue;
                    }
                    if !first_packet_received {
                        tracing::info!("[Player] First audio packet received from {}", r.1,);
                    }
                    last_packet_time = std::time::Instant::now();
                    first_packet_received = true;
                    r
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::UnexpectedEof
                        || e.kind() == std::io::ErrorKind::ConnectionReset
                    {
                        let _ = network_dropped_tx.try_send(());
                        break;
                    }
                    let timeout = 10;
                    let elapsed = last_packet_time.elapsed().as_secs();
                    if elapsed >= timeout {
                        tracing::warn!(
                            "[Player] Network timeout: no packets for {}s (threshold={}s), disconnecting",
                            elapsed,
                            timeout,
                        );
                        let _ = network_dropped_tx.try_send(());
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
            };

            streamer_port.store(streamer_addr.port(), Ordering::Relaxed);

            if let Some(tx) = streamer_ip_tx.take() {
                let _ = tx.send(streamer_addr.ip().to_string());
            }

            let Some(packet) = parse_packet(&recv_buff, len) else {
                continue;
            };

            let seq_num = packet.seq_num;
            let is_silence = packet.is_silence;
            let is_uncompressed = packet.is_uncompressed;

            if packet_producer.try_push(packet).is_err() {
                tracing::warn!(
                    "[WARN] SPSC ring buffer full, dropped seq {}. Audio callback may be stalled.",
                    seq_num
                );
            }

            if let Some(ref tx) = latency_tx
                && seq_num.is_multiple_of(100)
            {
                let rms_data = &recv_buff[SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE..len];
                let rms = compute_rms(rms_data, is_silence, is_uncompressed);
                let buffer_delay_ms = latency_metric.load(Ordering::Relaxed) as f32;
                let jitter_ms = jitter_metric.load(Ordering::Relaxed) as f32;
                let _ = tx.try_send((buffer_delay_ms, rms, jitter_ms));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::transport::AudioPacketTransport;
    use ringbuf::HeapRb;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32};
    use tokio::sync::mpsc;

    struct MockTransport {
        packet_to_send: Option<Vec<u8>>,
        streamer_addr: SocketAddr,
    }

    impl AudioPacketTransport for MockTransport {
        fn receive_audio_packet(
            &mut self,
            buffer: &mut [u8],
        ) -> std::io::Result<(usize, SocketAddr)> {
            if let Some(data) = self.packet_to_send.take() {
                let len = data.len();
                buffer[..len].copy_from_slice(&data);
                Ok((len, self.streamer_addr))
            } else {
                // Return EOF to terminate the loop
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Done",
                ))
            }
        }
    }

    #[tokio::test]
    async fn should_push_parsed_packet_to_ring_buffer_and_signal_network_drop() {
        let packet_rb = HeapRb::<RawPacket>::new(1024);
        let (producer, mut consumer) = packet_rb.split();
        let latency_metric = Arc::new(AtomicU32::new(0));
        let active = Arc::new(AtomicBool::new(true));
        let streamer_port = Arc::new(AtomicU16::new(0));
        let (network_dropped_tx, mut network_dropped_rx) = mpsc::channel(1);

        // Construct a dummy Opus packet
        let mut dummy_packet =
            vec![0u8; crate::audio::SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + 10];
        // Seq num = 42 (Big Endian)
        dummy_packet[0..8].copy_from_slice(&42u64.to_be_bytes());
        // Opus format flag
        dummy_packet[8] = crate::audio::FORMAT_OPUS;
        // payload = some data
        dummy_packet[9..19].copy_from_slice(&[0x1; 10]);

        let transport = MockTransport {
            packet_to_send: Some(dummy_packet),
            streamer_addr: "127.0.0.1:1234".parse().unwrap(),
        };

        let handle = spawn_packet_receive_thread(
            transport,
            producer,
            latency_metric,
            Arc::new(AtomicU32::new(0)),
            None,
            None,
            None,
            active,
            streamer_port,
            network_dropped_tx,
            Some("127.0.0.1".parse().unwrap()),
        );

        // Wait for thread to exit
        let _ = handle.join();

        // Ensure network drop was signaled due to EOF
        assert!(network_dropped_rx.recv().await.is_some());

        // Check if the packet was pushed to the consumer
        let received_packet = consumer.try_pop().expect("Packet should have been pushed");
        assert_eq!(received_packet.seq_num, 42);
        assert!(!received_packet.is_silence);
        assert!(!received_packet.is_uncompressed);
        assert_eq!(received_packet.payload_len, 10);
        assert_eq!(received_packet.payload_data[0], 0x1);
    }

    #[tokio::test]
    async fn should_ignore_udp_packets_from_a_different_streamer_ip() {
        let packet_rb = HeapRb::<RawPacket>::new(8);
        let (producer, mut consumer) = packet_rb.split();
        let active = Arc::new(AtomicBool::new(true));
        let streamer_port = Arc::new(AtomicU16::new(0));
        let (network_dropped_tx, mut network_dropped_rx) = mpsc::channel(1);
        let mut packet = vec![0u8; crate::audio::SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + 1];
        packet[0..8].copy_from_slice(&1u64.to_be_bytes());
        packet[8] = crate::audio::FORMAT_OPUS;

        let handle = spawn_packet_receive_thread(
            MockTransport {
                packet_to_send: Some(packet),
                streamer_addr: "10.0.0.2:23558".parse().unwrap(),
            },
            producer,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            None,
            None,
            None,
            active,
            streamer_port.clone(),
            network_dropped_tx,
            Some("10.0.0.1".parse().unwrap()),
        );

        handle.join().unwrap();
        assert!(network_dropped_rx.recv().await.is_some());
        assert!(consumer.try_pop().is_none());
        assert_eq!(streamer_port.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn should_reflect_an_echo_ping_to_the_rtt_channel_and_not_the_ring_buffer() {
        let packet_rb = HeapRb::<RawPacket>::new(8);
        let (producer, mut consumer) = packet_rb.split();
        let active = Arc::new(AtomicBool::new(true));
        let streamer_port = Arc::new(AtomicU16::new(0));
        let (network_dropped_tx, _network_dropped_rx) = mpsc::channel(1);
        let (rtt_tx, mut rtt_rx) = mpsc::channel::<f32>(4);

        let ping = crate::stream::echo::build_ping().to_vec();

        let handle = spawn_packet_receive_thread(
            MockTransport {
                packet_to_send: Some(ping),
                streamer_addr: "10.0.0.1:50000".parse().unwrap(),
            },
            producer,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            None,
            None,
            Some(rtt_tx),
            active,
            streamer_port.clone(),
            network_dropped_tx,
            Some("10.0.0.1".parse().unwrap()),
        );

        handle.join().unwrap();

        // An echo is liveness, not audio: nothing reaches the jitter ring and
        // the audio bookkeeping (streamer port) is untouched...
        assert!(consumer.try_pop().is_none());
        assert_eq!(streamer_port.load(Ordering::Relaxed), 0);
        // ...but a wire-RTT sample was emitted.
        let rtt = rtt_rx
            .try_recv()
            .expect("an RTT sample should have been sent");
        assert!(rtt >= 0.0, "rtt should be non-negative, got {rtt}");
    }
}
