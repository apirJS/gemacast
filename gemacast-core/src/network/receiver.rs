use crate::{
    audio::{
        MAX_OPUS_PACKET_SIZE, OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE, SEQ_NUM_SIZE,
        create_opus_decoder,
    },
    error::{AudioCaptureError, GemaCastError, NetworkError},
    jitter::{JitterBufferManager, RawPacket},
    network::AUDIO_PORT,
    types::JitterConfig,
};
use cpal::StreamError;
#[cfg(not(target_os = "android"))]
use cpal::traits::*;
#[cfg(target_os = "android")]
use oboe::{
    AudioOutputCallback, AudioOutputStreamSafe, AudioStream, AudioStreamBuilder, AudioStreamSafe,
    DataCallbackResult, PerformanceMode, SharingMode,
};
use ringbuf::{HeapProd, HeapRb, traits::*};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering},
    },
    time::Instant,
};
use tokio::sync::{mpsc, oneshot};

/// Capacity of the lock-free SPSC channel carrying raw packets
/// from the network thread to the cpal audio callback.
/// 1024 slots.
const PACKET_CHANNEL_CAPACITY: usize = 1024;

#[cfg(not(target_os = "android"))]
pub type PlaybackStream = cpal::Stream;

#[cfg(target_os = "android")]
pub type PlaybackStream = oboe::AudioStreamAsync<oboe::Output, OboeCallback>;

pub struct AudioReceiver {
    packet_producer: HeapProd<RawPacket>,
    playback_stream: PlaybackStream,
    error_rx: mpsc::Receiver<StreamError>,
    shutdown_rx: oneshot::Receiver<()>,
    latency_metric: Arc<AtomicU32>,
}

#[cfg(target_os = "android")]
pub struct OboeCallback {
    jitter_manager: JitterBufferManager,
    packet_consumer: ringbuf::HeapCons<RawPacket>,
    volume: Arc<AtomicU32>,
    is_playing: Arc<AtomicBool>,
}

#[cfg(target_os = "android")]
impl AudioOutputCallback for OboeCallback {
    type FrameType = (f32, oboe::Stereo);

    fn on_audio_ready(
        &mut self,
        _stream: &mut dyn AudioOutputStreamSafe,
        audio_data: &mut [(f32, f32)],
    ) -> DataCallbackResult {
        let vol = f32::from_bits(self.volume.load(Ordering::Relaxed));

        let float_slice = unsafe {
            std::slice::from_raw_parts_mut(
                audio_data.as_mut_ptr() as *mut f32,
                audio_data.len() * 2,
            )
        };

        if !self.is_playing.load(Ordering::Relaxed) {
            while self.packet_consumer.try_pop().is_some() {}
            for sample in float_slice.iter_mut() {
                *sample = 0.0;
            }
            self.jitter_manager.reset();
            return DataCallbackResult::Continue;
        }

        self.jitter_manager
            .ingest_packets(&mut self.packet_consumer);
        self.jitter_manager.fill_output(float_slice, vol);

        DataCallbackResult::Continue
    }
}

impl AudioReceiver {
    pub fn new(
        config_ref: Arc<std::sync::RwLock<JitterConfig>>,
        is_tcp_mode: Arc<AtomicBool>,
        is_playing: Arc<AtomicBool>,
        volume: Arc<AtomicU32>,
        #[allow(unused_variables)] exclusive_mode: bool,
        shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<Self, GemaCastError> {
        let (_error_tx, error_rx) = mpsc::channel::<StreamError>(1);
        let packet_rb = HeapRb::<RawPacket>::new(PACKET_CHANNEL_CAPACITY);
        let (packet_producer, packet_consumer) = packet_rb.split();
        let latency_metric = Arc::new(AtomicU32::new(0));

        #[cfg(not(target_os = "android"))]
        let playback_stream = build_cpal_stream(
            packet_consumer,
            config_ref,
            is_tcp_mode,
            is_playing,
            volume,
            latency_metric.clone(),
            _error_tx,
        )?;

        #[cfg(target_os = "android")]
        let playback_stream = build_oboe_stream(
            packet_consumer,
            config_ref,
            is_tcp_mode,
            is_playing,
            volume,
            latency_metric.clone(),
            exclusive_mode,
        )?;

        Ok(Self {
            packet_producer,
            playback_stream,
            error_rx,
            shutdown_rx,
            latency_metric,
        })
    }

    pub async fn start_audio_listener(
        mut self,
        sender_ip_tx: Option<oneshot::Sender<String>>,
        latency_tx: Option<mpsc::Sender<(f32, f32)>>,
        target_ip: Option<std::net::IpAddr>,
        mode: crate::types::ConnectionMode,
    ) -> Result<(), GemaCastError> {
        let (transport, heartbeat_socket) = setup_audio_transport(mode, target_ip)?;

        let heartbeat_active = Arc::new(AtomicBool::new(true));
        let sender_port = Arc::new(std::sync::atomic::AtomicU16::new(AUDIO_PORT));

        let heartbeat_thread = match (target_ip, heartbeat_socket) {
            (Some(target), Some(hb_socket)) => Some(spawn_heartbeat_thread(
                target,
                sender_port.clone(),
                heartbeat_active.clone(),
                hb_socket,
            )),
            _ => None,
        };

        let _playback_stream = self.playback_stream;

        let receiver_active = Arc::new(AtomicBool::new(true));

        let receiver_thread = spawn_receiver_thread(
            transport,
            self.packet_producer,
            self.latency_metric.clone(),
            sender_ip_tx,
            latency_tx,
            receiver_active.clone(),
            sender_port,
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
            Some(stream_err) = self.error_rx.recv() => {
                return Err(AudioCaptureError::StreamError(stream_err).into());
            }
            _ = &mut self.shutdown_rx => {}
        }

        Ok(())
    }

    pub fn start_audio_playback(&mut self) -> Result<(), GemaCastError> {
        #[cfg(not(target_os = "android"))]
        self.playback_stream
            .play()
            .map_err(AudioCaptureError::FailedToPlayOutputStream)?;

        #[cfg(target_os = "android")]
        {
            self.playback_stream.start().map_err(|_| {
                AudioCaptureError::FailedToPlayOutputStream(
                    cpal::PlayStreamError::DeviceNotAvailable,
                )
            })?;

            let burst = self.playback_stream.get_frames_per_burst();
            let _ = self.playback_stream.set_buffer_size_in_frames(burst * 2);
        }
        Ok(())
    }
}

#[cfg(not(target_os = "android"))]
fn build_cpal_stream(
    mut packet_consumer: ringbuf::HeapCons<RawPacket>,
    config_ref: Arc<std::sync::RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    latency_metric: Arc<AtomicU32>,
    error_tx: mpsc::Sender<StreamError>,
) -> Result<cpal::Stream, GemaCastError> {
    let decoder = create_opus_decoder().map_err(AudioCaptureError::OpusDecoderFailed)?;
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioCaptureError::DefaultOutputDeviceUnavailable)?;

    let mut buffer_size = cpal::BufferSize::Default;

    if let Ok(mut supported_configs) = device.supported_output_configs()
        && let Some(config) = supported_configs.find(|c| {
            c.channels() == OPUS_CHANNELS
                && c.min_sample_rate() <= OPUS_SAMPLE_RATE
                && c.max_sample_rate() >= OPUS_SAMPLE_RATE
        })
    {
        match config.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => {
                let desired = OPUS_FRAME_SAMPLES as u32;
                buffer_size = cpal::BufferSize::Fixed(desired.clamp(*min, *max));
            }
            cpal::SupportedBufferSize::Unknown => {}
        }
    }

    let stream_config = cpal::StreamConfig {
        channels: OPUS_CHANNELS,
        sample_rate: OPUS_SAMPLE_RATE,
        buffer_size,
    };

    let mut jitter_manager =
        JitterBufferManager::new(decoder, latency_metric, config_ref, is_tcp_mode);

    device
        .build_output_stream(
            &stream_config,
            move |data: &mut [f32], _: &_| {
                let vol = f32::from_bits(volume.load(Ordering::Relaxed));

                if !is_playing.load(Ordering::Relaxed) {
                    while packet_consumer.try_pop().is_some() {}
                    for sample in data.iter_mut() {
                        *sample = 0.0;
                    }
                    jitter_manager.reset();
                    return;
                }

                jitter_manager.ingest_packets(&mut packet_consumer);
                jitter_manager.fill_output(data, vol);
            },
            move |e| {
                let _ = error_tx.blocking_send(e);
            },
            None,
        )
        .map_err(|e| AudioCaptureError::FailedToBuildOutputStream(e).into())
}

#[cfg(target_os = "android")]
fn build_oboe_stream(
    packet_consumer: ringbuf::HeapCons<RawPacket>,
    config_ref: Arc<std::sync::RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    latency_metric: Arc<AtomicU32>,
    exclusive_mode: bool,
) -> Result<oboe::AudioStreamAsync<oboe::Output, OboeCallback>, GemaCastError> {
    let decoder = create_opus_decoder().map_err(AudioCaptureError::OpusDecoderFailed)?;
    let callback = OboeCallback {
        jitter_manager: JitterBufferManager::new(decoder, latency_metric, config_ref, is_tcp_mode),
        packet_consumer,
        volume,
        is_playing,
    };

    let builder = AudioStreamBuilder::default()
        .set_direction::<oboe::Output>()
        .set_performance_mode(PerformanceMode::LowLatency)
        .set_sharing_mode(if exclusive_mode {
            SharingMode::Exclusive
        } else {
            SharingMode::Shared
        })
        .set_format::<f32>()
        .set_channel_count::<oboe::Stereo>()
        .set_sample_rate(OPUS_SAMPLE_RATE as i32)
        .set_callback(callback);

    let stream = builder.open_stream().map_err(|_| {
        AudioCaptureError::FailedToBuildOutputStream(cpal::BuildStreamError::DeviceNotAvailable)
    })?;
    Ok(stream)
}

fn setup_audio_transport(
    mode: crate::types::ConnectionMode,
    target_ip: Option<std::net::IpAddr>,
) -> Result<
    (
        Box<dyn crate::network::transport::AudioTransport>,
        Option<std::net::UdpSocket>,
    ),
    NetworkError,
> {
    if mode == crate::types::ConnectionMode::Adb {
        let adb_addr = format!("127.0.0.1:{}", super::Ports::ADB_AUDIO_TCP);
        
        let stream_addr: std::net::SocketAddr = adb_addr.parse().unwrap();
        let stream = std::net::TcpStream::connect_timeout(
            &stream_addr,
            std::time::Duration::from_millis(2500),
        )
        .map_err(|source| NetworkError::TcpConnectFailed {
            addr: adb_addr.clone(),
            source,
        })?;
        let _ = stream.set_nodelay(true);
        
        let t = crate::network::transport::TcpTransport { stream };
        return Ok((Box::new(t), None));
    }

    let addr = std::net::SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, AUDIO_PORT));
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .map_err(|source| NetworkError::BindFailed {
        addr: addr.to_string(),
        source,
    })?;

    socket
        .set_reuse_address(true)
        .map_err(NetworkError::SetReuseAddressFailed)?;
    #[cfg(not(windows))]
    socket
        .set_reuse_port(true)
        .map_err(NetworkError::SetReusePortFailed)?;

    socket
        .bind(&addr.into())
        .map_err(|source| NetworkError::BindFailed {
            addr: addr.to_string(),
            source,
        })?;
    let std_socket: std::net::UdpSocket = socket.into();

    let cloned_for_tos = std_socket
        .try_clone()
        .map_err(NetworkError::SocketCloneFailed)?;
    socket2::Socket::from(cloned_for_tos)
        .set_tos(0xB8)
        .map_err(NetworkError::SetTosFailed)?;

    std_socket
        .set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .map_err(NetworkError::SetReadTimeoutFailed)?;

    if let Some(target) = target_ip {
        let target_addr = std::net::SocketAddr::new(target, AUDIO_PORT);
        std_socket
            .send_to(&[0u8], target_addr)
            .map_err(NetworkError::SendFailed)?;
        std_socket
            .send_to(&[0u8], target_addr)
            .map_err(NetworkError::SendFailed)?;
    }

    let heartbeat_socket = std_socket
        .try_clone()
        .map_err(NetworkError::SocketCloneFailed)?;
    Ok((
        Box::new(crate::network::transport::UdpTransport { socket: std_socket }),
        Some(heartbeat_socket),
    ))
}

fn spawn_heartbeat_thread(
    target: std::net::IpAddr,
    port: Arc<AtomicU16>,
    active: Arc<AtomicBool>,
    socket: std::net::UdpSocket,
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

fn spawn_receiver_thread(
    mut transport: Box<dyn crate::network::transport::AudioTransport>,
    mut packet_producer: HeapProd<RawPacket>,
    latency_metric: Arc<AtomicU32>,
    mut sender_ip_tx: Option<oneshot::Sender<String>>,
    latency_tx: Option<mpsc::Sender<(f32, f32)>>,
    active: Arc<AtomicBool>,
    sender_port: Arc<AtomicU16>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        #[cfg(target_os = "android")]
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, -19);
            libc::prctl(29, 1);
        }

        let mut recv_buff =
            vec![0u8; SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE];
        let mut first_packet_logged = false;

        while active.load(Ordering::Relaxed) {
            let result = transport.receive_packet(&mut recv_buff);
            let (len, sender_addr) = match result {
                Ok(r) => {
                    if !first_packet_logged {
                        
                        first_packet_logged = true;
                    }
                    r
                }
                Err(ref _e) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
            };

            sender_port.store(sender_addr.port(), Ordering::Relaxed);

            if let Some(tx) = sender_ip_tx.take() {
                let _ = tx.send(sender_addr.ip().to_string());
            }

            if len < SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE {
                continue;
            }

            let seq_bytes: [u8; 8] = recv_buff[..SEQ_NUM_SIZE].try_into().unwrap();
            let seq_num = u64::from_be_bytes(seq_bytes);

            let format_flag = recv_buff[SEQ_NUM_SIZE];
            let is_uncompressed = format_flag == crate::audio::FORMAT_UNCOMPRESSED;
            let is_silence = format_flag == crate::audio::FORMAT_SILENCE;

            let payload_len = len - (SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE);
            let payload_data = if payload_len > 0 {
                recv_buff[SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE..len].to_vec()
            } else {
                Vec::new()
            };

            let packet = RawPacket {
                seq_num,
                payload_data,
                payload_len,
                arrival_time: Instant::now(),
                is_uncompressed,
                is_silence,
            };
            let _ = packet_producer.try_push(packet);

            if let Some(ref tx) = latency_tx
                && seq_num.is_multiple_of(100)
            {
                let ms_per_frame =
                    (OPUS_FRAME_SAMPLES as f32 / OPUS_CHANNELS as f32 / OPUS_SAMPLE_RATE as f32)
                        * 1000.0;
                let rms_data = &recv_buff[SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE..len];
                let mut rms = 0.0f32;

                if is_silence {
                    rms = 0.0;
                } else if is_uncompressed {
                    let mut sum_sq = 0.0f32;
                    let mut count = 0;
                    for chunk in rms_data.chunks_exact(4) {
                        let f = f32::from_ne_bytes(chunk.try_into().unwrap());
                        sum_sq += f * f;
                        count += 1;
                    }
                    if count > 0 {
                        rms = (sum_sq / count as f32).sqrt();
                    }
                } else {
                    let bitrate_bytes_per_sec = crate::audio::OPUS_BITRATE as f32 / 8.0;
                    let typical_max = bitrate_bytes_per_sec * ms_per_frame / 1000.0;
                    rms = (rms_data.len() as f32 / typical_max).min(1.0).sqrt();
                }

                let jitter_delay_ms = latency_metric.load(Ordering::Relaxed) as f32;
                let total_latency_ms = jitter_delay_ms;
                let _ = tx.try_send((total_latency_ms, rms));
            }
        }
    })
}
