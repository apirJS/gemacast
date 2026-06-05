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
use crate::types::{AudioSource, TargetId};

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
    _audio_broadcast_tx: broadcast::Sender<Arc<Vec<u8>>>,
}

pub struct AudioCaptureInstance {
    /// Per-target encoders keyed by socket address (for UDP/WiFi targets).
    per_target_encoders: HashMap<SocketAddr, PerTargetEncoder>,
    /// TCP/ADB encoders keyed by DeviceId.
    tcp_encoders: HashMap<crate::types::DeviceId, TcpEncoder>,
    /// Broadcast channel for raw PCM frames from the capture thread.
    pcm_broadcast_tx: broadcast::Sender<Arc<Vec<f32>>>,
    pub capture_command_tx: mpsc::Sender<CaptureCommand>,
    pub capture_shutdown_tx: Option<oneshot::Sender<()>>,
    pub capture_join_handle: tokio::task::JoinHandle<()>,
}

impl AudioCaptureInstance {
    pub fn new(capture: CaptureHandle) -> Result<Self, GemaCastError> {
        let (pcm_broadcast_tx, _) = broadcast::channel(4000);
        let (capture_command_tx, capture_command_rx) = mpsc::channel(32);
        let (capture_shutdown_tx, capture_shutdown_rx) = oneshot::channel();
        let pcm_tx_clone = pcm_broadcast_tx.clone();

        let join_handle = tokio::spawn(async move {
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
            tcp_encoders: HashMap::new(),
            pcm_broadcast_tx,
            capture_command_tx,
            capture_shutdown_tx: Some(capture_shutdown_tx),
            capture_join_handle: join_handle,
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
    /// encodes at the given bitrate, and returns the dedicated broadcast channel.
    async fn spawn_tcp_encoder(
        &mut self,
        device_id: crate::types::DeviceId,
        bitrate: Option<i32>,
    ) -> Result<broadcast::Sender<Arc<Vec<u8>>>, GemaCastError> {
        self.remove_tcp_encoder(&device_id).await;

        let pcm_rx = self.pcm_broadcast_tx.subscribe();
        let (audio_broadcast_tx, _) = broadcast::channel(4000);
        let tcp_broadcast_tx = audio_broadcast_tx.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let join_handle = tokio::spawn(async move {
            let _ = run_tcp_encode_loop(pcm_rx, bitrate, tcp_broadcast_tx, shutdown_rx).await;
        });

        self.tcp_encoders.insert(device_id, TcpEncoder {
            _bitrate: bitrate,
            shutdown_tx,
            join_handle,
            _audio_broadcast_tx: audio_broadcast_tx.clone(),
        });

        Ok(audio_broadcast_tx)
    }

    /// Removes a per-target encoder, shutting down its task.
    async fn remove_target_encoder(&mut self, target_addr: &SocketAddr) {
        if let Some(encoder) = self.per_target_encoders.remove(target_addr) {
            let _ = encoder.shutdown_tx.send(());
            let _ = encoder.join_handle.await;
        }
    }

    /// Removes a TCP encoder for a specific device, shutting down its task.
    async fn remove_tcp_encoder(&mut self, device_id: &crate::types::DeviceId) {
        if let Some(encoder) = self.tcp_encoders.remove(device_id) {
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
    let dummy_encoder = crate::audio::create_opus_encoder().unwrap_or_else(|e| {
        tracing::error!("Fatal error: dummy encoder creation failed: {}", e);
        panic!("dummy encoder creation should never fail");
    });
    dummy_encoder
}

use super::capture::CaptureFactory;

pub struct CapturePool {
    instances: HashMap<AudioSource, AudioCaptureInstance>,
    max_instances: usize,
    pub supports_process_capture: bool,
    factory: Box<dyn CaptureFactory>,
}

impl CapturePool {
    pub fn new(factory: Box<dyn CaptureFactory>, supports_process_capture: bool) -> Self {
        Self {
            instances: HashMap::new(),
            max_instances: 8,
            supports_process_capture,
            factory,
        }
    }

    pub async fn subscribe(
        &mut self,
        source: AudioSource,
        target: TargetId,
        bitrate: Option<i32>,
    ) -> Result<Option<broadcast::Sender<Arc<Vec<u8>>>>, GemaCastError> {
        if !self.instances.contains_key(&source) {
            if self.instances.len() >= self.max_instances {
                return Err(AudioError::CapturePoolExhausted {
                    max: self.max_instances,
                }
                .into());
            }

            let handle = match &source {
                AudioSource::Desktop => self.factory.create_desktop_capture()?,
                AudioSource::Process { pid, .. } => {
                    if !self.supports_process_capture {
                        return Err(AudioError::ProcessCaptureUnavailable.into());
                    }
                    self.factory.create_process_capture(*pid)?
                }
            };

            let instance = AudioCaptureInstance::new(handle)?;
            self.instances.insert(source.clone(), instance);
        }

        let instance = self.instances.get_mut(&source).unwrap();
        let ret = match target {
            TargetId::Udp(addr) => {
                instance.spawn_target_encoder(addr, bitrate).await?;
                let _ = instance
                    .capture_command_tx
                    .send(CaptureCommand::AddTarget { addr, bitrate })
                    .await;
                None
            }
            TargetId::Tcp(device_id) => {
                Some(instance.spawn_tcp_encoder(device_id, bitrate).await?)
            }
        };

        Ok(ret)
    }

    pub async fn unsubscribe(
        &mut self,
        source: &AudioSource,
        target: TargetId,
    ) -> Result<(), GemaCastError> {
        if let Some(instance) = self.instances.get_mut(source) {
            match target {
                TargetId::Udp(addr) => {
                    instance.remove_target_encoder(&addr).await;
                    let _ = instance
                        .capture_command_tx
                        .send(CaptureCommand::RemoveTarget(addr))
                        .await;
                }
                TargetId::Tcp(device_id) => {
                    instance.remove_tcp_encoder(&device_id).await;
                }
            }

            let is_teardown_eligible = true;
            if is_teardown_eligible
                && instance.per_target_encoders.is_empty()
                && instance.tcp_encoders.is_empty()
                && let Some(mut removed) = self.instances.remove(source)
                && let Some(stop_tx) = removed.capture_shutdown_tx.take()
            {
                let _ = stop_tx.send(());
                let _ = removed.capture_join_handle.await;
            }
        }
        Ok(())
    }

    pub async fn change_source(
        &mut self,
        old_source: &AudioSource,
        new_source: AudioSource,
        target: TargetId,
        bitrate: Option<i32>,
    ) -> Result<Option<broadcast::Sender<Arc<Vec<u8>>>>, GemaCastError> {
        if old_source == &new_source {
            return self.subscribe(new_source, target, bitrate).await;
        }

        let tx = self.subscribe(new_source, target.clone(), bitrate).await?;
        let _ = self.unsubscribe(old_source, target).await;
        Ok(tx)
    }

    pub async fn change_bitrate(
        &mut self,
        source: &AudioSource,
        target: TargetId,
        bitrate: Option<i32>,
    ) -> Result<Option<broadcast::Sender<Arc<Vec<u8>>>>, GemaCastError> {
        if let Some(instance) = self.instances.get_mut(source) {
            match target {
                TargetId::Udp(addr) => {
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
                    Ok(None)
                }
                TargetId::Tcp(device_id) => {
                    instance.remove_tcp_encoder(&device_id).await;
                    let tx = instance.spawn_tcp_encoder(device_id, bitrate).await?;
                    Ok(Some(tx))
                }
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DeviceId;
    use crate::stream::sender::capture::CaptureBackend;
    use ringbuf::traits::*;
    use ringbuf::HeapRb;
    use tokio::sync::Notify;

    struct MockBackend;
    impl CaptureBackend for MockBackend {
        fn play(&mut self) -> Result<(), GemaCastError> {
            Ok(())
        }
        fn pause(&mut self) -> Result<(), GemaCastError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn capture_instance_should_broadcast_pcm_and_encode_tcp() {
        // 1. Setup ringbuffer and mock handle
        let ring_buffer = HeapRb::<f32>::new(48000 * 2);
        let (mut producer, consumer) = ring_buffer.split();
        let notify = Arc::new(Notify::new());
        let (_err_tx, err_rx) = mpsc::channel(1);

        let capture_handle = CaptureHandle {
            backend: Box::new(MockBackend),
            consumer,
            notify: notify.clone(),
            stream_error_rx: err_rx,
        };

        // 2. Create the AudioCaptureInstance
        let mut instance = AudioCaptureInstance::new(capture_handle)
            .expect("Failed to create AudioCaptureInstance");

        // We can manually subscribe to the internal PCM broadcast channel to verify the capture loop
        let mut pcm_rx = instance.pcm_broadcast_tx.subscribe();

        // 3. Push fake PCM data
        // Opus stereo encoding expects exactly OPUS_FRAME_SAMPLES
        let frame_size = crate::audio::OPUS_FRAME_SAMPLES;
        let fake_audio = vec![0.5f32; frame_size];

        producer.push_slice(&fake_audio);
        notify.notify_one();

        // 4. Verify capture loop reads and broadcasts PCM (ignoring any silence watchdog frames)
        let mut received_pcm;
        loop {
            received_pcm = pcm_rx.recv().await.unwrap_or_else(|_| {
                tracing::error!("Fatal error: Failed to receive PCM");
                panic!("Failed to receive PCM");
            });
            if received_pcm[0] != 0.0 {
                break;
            }
        }
        assert_eq!(received_pcm.len(), frame_size);
        assert_eq!(received_pcm[0], 0.5f32);

        // 5. Test Encoder spawning
        let device_id = DeviceId("test_dev".into());
        let audio_broadcast_tx = instance
            .spawn_tcp_encoder(device_id.clone(), Some(128000))
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Fatal error: Failed to spawn TCP encoder: {}", e);
                panic!("Failed to spawn TCP encoder: {}", e);
            });

        let mut encoded_rx = audio_broadcast_tx.subscribe();
        
        // Push another frame so the encoder has something to encode
        producer.push_slice(&fake_audio);
        notify.notify_one();

        // The encoder should eventually emit an opus packet (ignore silence watchdog packets)
        let mut encoded_packet;
        loop {
            encoded_packet = encoded_rx.recv().await.unwrap_or_else(|_| {
                tracing::error!("Fatal error: Failed to receive Opus packet");
                panic!("Failed to receive Opus packet");
            });
            // Verify packet contains sequence number (8 bytes) + format flag (1 byte) + some opus payload
            if encoded_packet.len() > 9 {
                break;
            }
        }

        // 6. Test clean asynchronous teardown (simulating unsubscribe)
        instance.remove_tcp_encoder(&device_id).await;
        if let Some(stop_tx) = instance.capture_shutdown_tx.take() {
            stop_tx.send(()).unwrap();
            // Await the join handle like unsubscribe does, verifying no deadlocks!
            instance.capture_join_handle.await.unwrap_or_else(|e| {
                tracing::error!("Fatal error: Capture loop panicked: {}", e);
                panic!("Capture loop panicked");
            });
        }
    }

    #[tokio::test]
    async fn capture_instance_should_encode_and_send_udp_packets() {
        // 1. Setup ringbuffer and mock handle
        let ring_buffer = HeapRb::<f32>::new(48000 * 2);
        let (mut producer, consumer) = ring_buffer.split();
        let notify = Arc::new(Notify::new());
        let (_err_tx, err_rx) = mpsc::channel(1);

        let capture_handle = CaptureHandle {
            backend: Box::new(MockBackend),
            consumer,
            notify: notify.clone(),
            stream_error_rx: err_rx,
        };

        // 2. Create the AudioCaptureInstance
        let mut instance = AudioCaptureInstance::new(capture_handle)
            .expect("Failed to create AudioCaptureInstance");

        // Bind a local UDP socket to receive the encoded packets
        let receiver_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target_addr = receiver_socket.local_addr().unwrap();

        // 3. Spawn UDP target encoder
        instance
            .spawn_target_encoder(target_addr, Some(128000))
            .await
            .expect("Failed to spawn UDP encoder");

        // Push fake audio frame
        let frame_size = crate::audio::OPUS_FRAME_SAMPLES;
        let fake_audio = vec![0.5f32; frame_size];

        producer.push_slice(&fake_audio);
        notify.notify_one();

        // The encoder should eventually emit an opus packet over UDP
        let mut buf = vec![0u8; 1500];
        let mut len;
              
        loop {
            // Keep pushing audio because UDP packets could be dropped (e.g. ARP/startup delays)
            producer.push_slice(&fake_audio);
            notify.notify_one();

            let recv_future = receiver_socket.recv_from(&mut buf);
            let (recv_len, _) = tokio::time::timeout(std::time::Duration::from_millis(500), recv_future)
                .await
                .expect("Timed out waiting for UDP packet")
                .expect("Failed to receive UDP packet");
            
            len = recv_len;
            if len > 9 {
                break;
            }
        }

        // Verify packet contains sequence number (8 bytes) + format flag (1 byte) + some opus payload
        assert!(len > 9);

        // 4. Test clean teardown
        instance.remove_target_encoder(&target_addr).await;
        if let Some(stop_tx) = instance.capture_shutdown_tx.take() {
            stop_tx.send(()).unwrap();
            instance.capture_join_handle.await.expect("Capture loop panicked");
        }
    }

    struct MockCaptureFactory;

    impl CaptureFactory for MockCaptureFactory {
        fn create_desktop_capture(&self) -> Result<CaptureHandle, GemaCastError> {
            let ring_buffer = HeapRb::<f32>::new(48000 * 2);
            let (_producer, consumer) = ring_buffer.split();
            let notify = Arc::new(Notify::new());
            let (_err_tx, err_rx) = mpsc::channel(1);

            Ok(CaptureHandle {
                backend: Box::new(MockBackend),
                consumer,
                notify,
                stream_error_rx: err_rx,
            })
        }

        fn create_process_capture(&self, _pid: u32) -> Result<CaptureHandle, GemaCastError> {
            self.create_desktop_capture() // Just reuse the mock for tests
        }
    }

    #[tokio::test]
    async fn pool_should_create_and_teardown_instances_on_subscribe_unsubscribe() {
        let factory = Box::new(MockCaptureFactory);
        let mut pool = CapturePool::new(factory, true);
        let target = TargetId::Tcp(DeviceId("dev1".into()));

        // 1. Subscribe to desktop
        let _tx = pool.subscribe(AudioSource::Desktop, target.clone(), Some(128000)).await.expect("Subscribe failed");
        assert_eq!(pool.instances.len(), 1);

        // 2. Subscribe again (should reuse the instance)
        let _tx2 = pool.subscribe(AudioSource::Desktop, target.clone(), Some(128000)).await.expect("Subscribe failed");
        assert_eq!(pool.instances.len(), 1);

        // 3. Unsubscribe (should teardown)
        pool.unsubscribe(&AudioSource::Desktop, target).await.expect("Unsubscribe failed");
        assert_eq!(pool.instances.len(), 0);
    }

    #[tokio::test]
    async fn pool_should_migrate_target_when_changing_source() {
        let factory = Box::new(MockCaptureFactory);
        let mut pool = CapturePool::new(factory, true);
        let target = TargetId::Tcp(DeviceId("dev1".into()));

        // Subscribe to desktop
        pool.subscribe(AudioSource::Desktop, target.clone(), Some(128000)).await.expect("Subscribe failed");
        assert_eq!(pool.instances.len(), 1);

        // Change source to process
        let new_source = AudioSource::Process { pid: 1234, name: "test".into() };
        pool.change_source(&AudioSource::Desktop, new_source.clone(), target, Some(128000)).await.expect("Change source failed");
        
        // Old instance should be gone, new instance should be created
        assert_eq!(pool.instances.len(), 1);
        assert!(pool.instances.contains_key(&new_source));
    }

    #[tokio::test]
    async fn pool_should_support_multiple_tcp_encoders_per_source() {
        let factory = Box::new(MockCaptureFactory);
        let mut pool = CapturePool::new(factory, true);
        let target1 = TargetId::Tcp(DeviceId("dev1".into()));
        let target2 = TargetId::Tcp(DeviceId("dev2".into()));

        pool.subscribe(AudioSource::Desktop, target1.clone(), Some(128000)).await.expect("Subscribe 1 failed");
        pool.subscribe(AudioSource::Desktop, target2.clone(), Some(256000)).await.expect("Subscribe 2 failed");

        let instance = pool.instances.get(&AudioSource::Desktop).unwrap();
        assert_eq!(instance.tcp_encoders.len(), 2);

        pool.unsubscribe(&AudioSource::Desktop, target1).await.expect("Unsubscribe 1 failed");
        
        // Teardown should not happen yet
        assert_eq!(pool.instances.len(), 1);
        let instance = pool.instances.get(&AudioSource::Desktop).unwrap();
        assert_eq!(instance.tcp_encoders.len(), 1);

        pool.unsubscribe(&AudioSource::Desktop, target2).await.expect("Unsubscribe 2 failed");
        
        // Now it should teardown
        assert_eq!(pool.instances.len(), 0);
    }

    #[tokio::test]
    async fn pool_should_support_multiple_udp_encoders_per_source() {
        let factory = Box::new(MockCaptureFactory);
        let mut pool = CapturePool::new(factory, true);
        
        let target1 = TargetId::Udp("127.0.0.1:1111".parse().unwrap());
        let target2 = TargetId::Udp("127.0.0.1:2222".parse().unwrap());

        pool.subscribe(AudioSource::Desktop, target1.clone(), Some(128000)).await.expect("Subscribe 1 failed");
        pool.subscribe(AudioSource::Desktop, target2.clone(), Some(256000)).await.expect("Subscribe 2 failed");

        let instance = pool.instances.get(&AudioSource::Desktop).unwrap();
        assert_eq!(instance.per_target_encoders.len(), 2);

        pool.unsubscribe(&AudioSource::Desktop, target1).await.expect("Unsubscribe 1 failed");
        
        // Teardown should not happen yet
        assert_eq!(pool.instances.len(), 1);
        let instance = pool.instances.get(&AudioSource::Desktop).unwrap();
        assert_eq!(instance.per_target_encoders.len(), 1);

        pool.unsubscribe(&AudioSource::Desktop, target2).await.expect("Unsubscribe 2 failed");
        
        // Now it should teardown
        assert_eq!(pool.instances.len(), 0);
    }
}
