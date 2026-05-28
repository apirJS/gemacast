use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::capture::CaptureHandle;
use super::encode::{EncodeResult, encode_frame};
use super::engine::CaptureCommand;
use crate::audio::{
    MAX_OPUS_PACKET_SIZE, OPUS_FRAME_SAMPLES, SEQ_NUM_SIZE, create_opus_encoder_with_bitrate,
};
use crate::error::{AudioError, CodecDirection, GemaCastError, NetworkError};
use crate::types::AudioSource;

/// Tracks one per-target encoder task. Each connected receiver gets its own encoder
/// at its requested bitrate, running in a dedicated tokio task.
struct PerTargetEncoder {
    _bitrate: Option<i32>,
    shutdown_tx: oneshot::Sender<()>,
    join_handle: tokio::task::JoinHandle<()>,
}

/// Tracks a TCP/ADB encoder that publishes to the broadcast channel
/// instead of sending UDP packets.
struct TcpEncoder {
    _bitrate: Option<i32>,
    shutdown_tx: oneshot::Sender<()>,
    join_handle: tokio::task::JoinHandle<()>,
}

pub struct AudioCaptureInstance {
    /// Per-target encoders keyed by socket address (for UDP/WiFi targets).
    per_target_encoders: HashMap<SocketAddr, PerTargetEncoder>,
    /// TCP/ADB encoder that publishes to `audio_broadcast_tx`.
    tcp_encoder: Option<TcpEncoder>,
    /// Broadcast channel for raw PCM frames from the capture thread.
    pcm_broadcast_tx: broadcast::Sender<Arc<Vec<f32>>>,
    /// Broadcast channel for encoded packets (consumed by TCP/ADB spigots).
    pub audio_broadcast_tx: broadcast::Sender<Arc<Vec<u8>>>,
    pub capture_command_tx: mpsc::Sender<CaptureCommand>,
    pub capture_shutdown_tx: Option<oneshot::Sender<()>>,
}

impl AudioCaptureInstance {
    pub fn new(capture: CaptureHandle) -> Result<Self, GemaCastError> {
        let (pcm_broadcast_tx, _) = broadcast::channel(4000);
        let (audio_broadcast_tx, _) = broadcast::channel(4000);
        let (capture_command_tx, capture_command_rx) = mpsc::channel(32);
        let (capture_shutdown_tx, capture_shutdown_rx) = oneshot::channel();
        let pcm_tx_clone = pcm_broadcast_tx.clone();

        tokio::spawn(async move {
            let _ = Self::run_capture_loop(
                capture,
                capture_command_rx,
                capture_shutdown_rx,
                pcm_tx_clone,
            )
            .await;
        });

        Ok(Self {
            per_target_encoders: HashMap::new(),
            tcp_encoder: None,
            pcm_broadcast_tx,
            audio_broadcast_tx,
            capture_command_tx,
            capture_shutdown_tx: Some(capture_shutdown_tx),
        })
    }

    /// Spawns a per-target encoder task that subscribes to raw PCM frames,
    /// encodes at the given bitrate, and sends UDP packets to the target.
    async fn spawn_target_encoder(
        &mut self,
        target_addr: SocketAddr,
        bitrate: Option<i32>,
    ) -> Result<(), GemaCastError> {
        self.remove_target_encoder(&target_addr).await;

        let pcm_rx = self.pcm_broadcast_tx.subscribe();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let join_handle = tokio::spawn(async move {
            let _ = run_per_target_encode_loop(pcm_rx, target_addr, bitrate, shutdown_rx).await;
        });

        self.per_target_encoders.insert(
            target_addr,
            PerTargetEncoder {
                _bitrate: bitrate,
                shutdown_tx,
                join_handle,
            },
        );
        Ok(())
    }

    /// Spawns a TCP encoder task that subscribes to raw PCM frames,
    /// encodes at the given bitrate, and publishes to `audio_broadcast_tx`.
    async fn spawn_tcp_encoder(&mut self, bitrate: Option<i32>) -> Result<(), GemaCastError> {
        self.remove_tcp_encoder().await;

        let pcm_rx = self.pcm_broadcast_tx.subscribe();
        let tcp_broadcast_tx = self.audio_broadcast_tx.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let join_handle = tokio::spawn(async move {
            let _ = run_tcp_encode_loop(pcm_rx, bitrate, tcp_broadcast_tx, shutdown_rx).await;
        });

        self.tcp_encoder = Some(TcpEncoder {
            _bitrate: bitrate,
            shutdown_tx,
            join_handle,
        });
        Ok(())
    }

    /// Removes a per-target encoder, shutting down its task.
    async fn remove_target_encoder(&mut self, target_addr: &SocketAddr) {
        if let Some(encoder) = self.per_target_encoders.remove(target_addr) {
            let _ = encoder.shutdown_tx.send(());
            let _ = encoder.join_handle.await;
        }
    }

    /// Removes the TCP encoder, shutting down its task.
    async fn remove_tcp_encoder(&mut self) {
        if let Some(encoder) = self.tcp_encoder.take() {
            let _ = encoder.shutdown_tx.send(());
            let _ = encoder.join_handle.await;
        }
    }

    /// The capture loop: reads raw PCM from the audio backend and broadcasts
    /// raw frames. No encoding happens here.
    async fn run_capture_loop(
        mut capture: CaptureHandle,
        mut capture_command_rx: mpsc::Receiver<CaptureCommand>,
        mut capture_shutdown_rx: oneshot::Receiver<()>,
        pcm_broadcast_tx: broadcast::Sender<Arc<Vec<f32>>>,
    ) -> Result<(), GemaCastError> {
        let mut targets: HashSet<SocketAddr> = HashSet::new();
        use ringbuf::traits::*;
        let mut sample_buf = Vec::<f32>::with_capacity(OPUS_FRAME_SAMPLES * 2);

        // Watchdog interval to inject silence if WASAPI loopback goes idle (e.g. no apps playing audio)
        // 22ms is slightly longer than the standard 20ms Opus frame duration.
        let mut silence_interval = tokio::time::interval(tokio::time::Duration::from_millis(22));
        silence_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        capture.backend.play()?;

        loop {
            tokio::select! {
                Some(command) = capture_command_rx.recv() => {
                    match command {
                        CaptureCommand::AddTarget { addr, .. } => {
                            targets.insert(addr);
                        }
                        CaptureCommand::RemoveTarget(target_addr) => {
                            targets.remove(&target_addr);
                            if targets.is_empty() && pcm_broadcast_tx.receiver_count() == 0 {
                                sample_buf.clear();
                            }
                        }
                    }
                },
                _ = capture.notify.notified() => {
                    // We received real audio, reset the silence watchdog
                    silence_interval.reset();

                    let occupied = capture.consumer.occupied_len();
                    if occupied == 0 {
                        continue;
                    }

                    // Produce frames if any UDP targets exist OR any PCM subscribers
                    // (per-target or TCP encoder tasks) are listening.
                    let has_pcm_listeners = pcm_broadcast_tx.receiver_count() > 0;
                    if targets.is_empty() && !has_pcm_listeners {
                        while capture.consumer.try_pop().is_some() {}
                        continue;
                    }

                    let prev_len = sample_buf.len();
                    sample_buf.resize(prev_len + occupied, 0.0);

                    let actually_read = capture.consumer.pop_slice(&mut sample_buf[prev_len..]);
                    sample_buf.truncate(prev_len + actually_read);

                    while sample_buf.len() >= OPUS_FRAME_SAMPLES {
                        let frame = Arc::new(sample_buf[..OPUS_FRAME_SAMPLES].to_vec());
                        sample_buf.drain(..OPUS_FRAME_SAMPLES);

                        // Broadcast raw PCM frame to all encoder tasks
                        let _ = pcm_broadcast_tx.send(frame);
                    }
                },
                _ = silence_interval.tick() => {
                    // No real audio received for 22ms. Inject a silent frame if anyone is listening
                    // to prevent the mobile client from timing out and disconnecting.
                    let has_pcm_listeners = pcm_broadcast_tx.receiver_count() > 0;
                    if !targets.is_empty() || has_pcm_listeners {
                        let silent_frame = Arc::new(vec![0.0f32; OPUS_FRAME_SAMPLES]);
                        let _ = pcm_broadcast_tx.send(silent_frame);
                    }
                },
                Some(stream_error) = capture.stream_error_rx.recv() => {
                    return Err(AudioError::StreamError(stream_error).into());
                },
                _ = &mut capture_shutdown_rx => {
                    let _ = capture.backend.pause();
                    break;
                }
                else => break,
            }
        }

        Ok(())
    }
}

/// Per-target UDP encoder loop: receives raw PCM frames, encodes at the
/// configured bitrate, and sends UDP packets to the target address.
async fn run_per_target_encode_loop(
    mut pcm_rx: broadcast::Receiver<Arc<Vec<f32>>>,
    target_addr: SocketAddr,
    current_bitrate: Option<i32>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), GemaCastError> {
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| {
        NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source: e,
        }
    })?;

    let _ = socket.set_tos_v4(0xB8);

    socket
        .bind(&addr.into())
        .map_err(|e| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

    socket
        .set_nonblocking(true)
        .map_err(|e| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

    let audio_socket =
        UdpSocket::from_std(socket.into()).map_err(|e| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

    // Create the encoder at this target's requested bitrate (or skip if uncompressed)
    let mut encoder = if current_bitrate.is_some() {
        let bitrate = current_bitrate.unwrap_or(128_000);
        Some(
            create_opus_encoder_with_bitrate(bitrate).map_err(|e| AudioError::OpusInitFailed {
                direction: CodecDirection::Encoder,
                source: e,
            })?,
        )
    } else {
        None
    };

    let mut seq_num: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut opus_output = vec![0u8; MAX_OPUS_PACKET_SIZE];
    let mut packet_buf: Vec<u8> =
        Vec::with_capacity(SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE);

    loop {
        tokio::select! {
            result = pcm_rx.recv() => {
                let frame = match result {
                    Ok(f) => f,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[PerTargetEncoder] Lagged by {} frames for {:?}", n, target_addr);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };

                // Drain any stale data from the socket
                let mut drain_buf = [0u8; 1];
                while audio_socket.try_recv(&mut drain_buf).is_ok() {}

                let result = encode_frame(
                    &frame,
                    encoder.as_mut().unwrap_or(&mut create_dummy_encoder()),
                    current_bitrate,
                    seq_num,
                    &mut opus_output,
                    &mut packet_buf,
                );

                if matches!(result, EncodeResult::Skipped) {
                    continue;
                }

                // Send UDP to target
                match audio_socket.try_send_to(&packet_buf, target_addr) {
                    Ok(_) => {}
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => {}
                }

                seq_num = seq_num.wrapping_add(1);
            }
            _ = &mut shutdown_rx => break,
        }
    }

    Ok(())
}

/// TCP encoder loop: receives raw PCM frames, encodes at the configured
/// bitrate, and publishes to the broadcast channel for TCP/ADB consumers.
async fn run_tcp_encode_loop(
    mut pcm_rx: broadcast::Receiver<Arc<Vec<f32>>>,
    current_bitrate: Option<i32>,
    tcp_broadcast_tx: broadcast::Sender<Arc<Vec<u8>>>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), GemaCastError> {
    // Create the encoder at this target's requested bitrate (or skip if uncompressed)
    let mut encoder = if current_bitrate.is_some() {
        let bitrate = current_bitrate.unwrap_or(128_000);
        Some(
            create_opus_encoder_with_bitrate(bitrate).map_err(|e| AudioError::OpusInitFailed {
                direction: CodecDirection::Encoder,
                source: e,
            })?,
        )
    } else {
        None
    };

    let mut seq_num: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut opus_output = vec![0u8; MAX_OPUS_PACKET_SIZE];
    let mut packet_buf: Vec<u8> =
        Vec::with_capacity(SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE);

    loop {
        tokio::select! {
            result = pcm_rx.recv() => {
                let frame = match result {
                    Ok(f) => f,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[TcpEncoder] Lagged by {} frames", n);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };

                let result = encode_frame(
                    &frame,
                    encoder.as_mut().unwrap_or(&mut create_dummy_encoder()),
                    current_bitrate,
                    seq_num,
                    &mut opus_output,
                    &mut packet_buf,
                );

                if matches!(result, EncodeResult::Skipped) {
                    continue;
                }

                let shared = Arc::new(packet_buf.clone());
                let _ = tcp_broadcast_tx.send(shared);

                seq_num = seq_num.wrapping_add(1);
            }
            _ = &mut shutdown_rx => break,
        }
    }

    Ok(())
}

/// Creates a dummy encoder that is never actually used — only exists to satisfy
/// the borrow checker when current_bitrate is None (uncompressed mode).
fn create_dummy_encoder() -> opus::Encoder {
    opus::Encoder::new(
        crate::audio::OPUS_SAMPLE_RATE,
        opus::Channels::Stereo,
        opus::Application::LowDelay,
    )
    .expect("dummy encoder creation should never fail")
}

pub struct CapturePool {
    instances: HashMap<AudioSource, AudioCaptureInstance>,
    max_instances: usize,
    pub supports_process_capture: bool,
}

impl CapturePool {
    pub fn new(supports_process_capture: bool) -> Self {
        Self {
            instances: HashMap::new(),
            max_instances: 8,
            supports_process_capture,
        }
    }

    pub async fn subscribe(
        &mut self,
        source: AudioSource,
        target_addr: Option<SocketAddr>,
        bitrate: Option<i32>,
    ) -> Result<broadcast::Sender<Arc<Vec<u8>>>, GemaCastError> {
        if !self.instances.contains_key(&source) {
            if self.instances.len() >= self.max_instances {
                return Err(AudioError::CapturePoolExhausted {
                    max: self.max_instances,
                }
                .into());
            }

            let handle = match &source {
                AudioSource::Desktop => {
                    #[cfg(windows)]
                    {
                        super::capture::wasapi_desktop::create_wasapi_desktop_loopback()?
                    }
                    #[cfg(not(windows))]
                    {
                        super::capture::cpal_loopback::create_cpal_loopback()?
                    }
                }
                #[allow(unused_variables)]
                AudioSource::Process { pid, .. } => {
                    if !self.supports_process_capture {
                        return Err(AudioError::ProcessCaptureUnavailable.into());
                    }
                    #[cfg(windows)]
                    {
                        super::capture::wasapi_loopback::create_wasapi_process_loopback(*pid)?
                    }
                    #[cfg(not(windows))]
                    {
                        return Err(AudioError::ProcessCaptureUnavailable.into());
                    }
                }
            };

            let instance = AudioCaptureInstance::new(handle)?;
            self.instances.insert(source.clone(), instance);
        }

        let instance = self.instances.get_mut(&source).unwrap();
        if let Some(addr) = target_addr {
            // Spawn a per-target UDP encoder at this receiver's bitrate
            instance.spawn_target_encoder(addr, bitrate).await?;
            let _ = instance
                .capture_command_tx
                .send(CaptureCommand::AddTarget { addr, bitrate })
                .await;
        } else {
            // TCP/ADB: spawn a TCP encoder that publishes to the broadcast channel
            instance.spawn_tcp_encoder(bitrate).await?;
        }

        Ok(instance.audio_broadcast_tx.clone())
    }

    pub async fn unsubscribe(
        &mut self,
        source: &AudioSource,
        target_addr: Option<SocketAddr>,
    ) -> Result<(), GemaCastError> {
        if let Some(instance) = self.instances.get_mut(source) {
            if let Some(addr) = target_addr {
                instance.remove_target_encoder(&addr).await;
                let _ = instance
                    .capture_command_tx
                    .send(CaptureCommand::RemoveTarget(addr))
                    .await;
            } else {
                instance.remove_tcp_encoder().await;
            }

            let is_teardown_eligible = true;
            if is_teardown_eligible
                && instance.per_target_encoders.is_empty()
                && instance.tcp_encoder.is_none()
                && let Some(mut removed) = self.instances.remove(source)
                && let Some(stop_tx) = removed.capture_shutdown_tx.take()
            {
                let _ = stop_tx.send(());
            }
        }
        Ok(())
    }

    pub async fn change_source(
        &mut self,
        old_source: &AudioSource,
        new_source: AudioSource,
        target_addr: Option<SocketAddr>,
        bitrate: Option<i32>,
    ) -> Result<broadcast::Sender<Arc<Vec<u8>>>, GemaCastError> {
        // Try subscribing to the new source FIRST.
        // This ensures that if the new source fails (e.g. process exited),
        // we don't accidentally tear down the currently working old_source!
        let tx = self.subscribe(new_source, target_addr, bitrate).await?;
        let _ = self.unsubscribe(old_source, target_addr).await;
        Ok(tx)
    }

    pub async fn change_bitrate(
        &mut self,
        source: &AudioSource,
        target_addr: Option<SocketAddr>,
        bitrate: Option<i32>,
    ) -> Result<broadcast::Sender<Arc<Vec<u8>>>, GemaCastError> {
        if let Some(instance) = self.instances.get_mut(source) {
            if let Some(addr) = target_addr {
                instance.remove_target_encoder(&addr).await;
                let _ = instance
                    .capture_command_tx
                    .send(CaptureCommand::RemoveTarget(addr))
                    .await;
                instance.spawn_target_encoder(addr, bitrate).await?;
                let _ = instance
                    .capture_command_tx
                    .send(CaptureCommand::AddTarget { addr, bitrate })
                    .await;
            } else {
                instance.remove_tcp_encoder().await;
                instance.spawn_tcp_encoder(bitrate).await?;
            }
            Ok(instance.audio_broadcast_tx.clone())
        } else {
            Err(AudioError::SourceNotSubscribed.into())
        }
    }

    pub fn available_sources(&self) -> Result<Vec<AudioSource>, GemaCastError> {
        let mut sources = vec![AudioSource::Desktop];

        for source in self.instances.keys() {
            if let AudioSource::Process { .. } = source {
                sources.push(source.clone());
            }
        }

        Ok(sources)
    }

    pub fn get_tcp_broadcaster(
        &self,
        source: &AudioSource,
    ) -> Option<broadcast::Sender<Arc<Vec<u8>>>> {
        self.instances
            .get(source)
            .map(|i| i.audio_broadcast_tx.clone())
    }
}
