use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::capture::CaptureHandle;
use super::encode::{EncodeResult, encode_frame};
use super::engine::CaptureCommand;
use crate::audio::{MAX_OPUS_PACKET_SIZE, OPUS_FRAME_SAMPLES, SEQ_NUM_SIZE, create_opus_encoder};
use crate::error::{AudioError, CodecDirection, GemaCastError, NetworkError};
use crate::types::AudioSource;

pub struct AudioCaptureInstance {
    pub targets: HashSet<SocketAddr>,
    pub audio_broadcast_tx: broadcast::Sender<Arc<Vec<u8>>>,
    pub capture_command_tx: mpsc::Sender<CaptureCommand>,
    pub capture_shutdown_tx: Option<oneshot::Sender<()>>,
}

impl AudioCaptureInstance {
    pub fn new(capture: CaptureHandle) -> Result<Self, GemaCastError> {
        let (audio_broadcast_tx, _) = broadcast::channel(4000);
        let (capture_command_tx, capture_command_rx) = mpsc::channel(32);
        let (capture_shutdown_tx, capture_shutdown_rx) = oneshot::channel();
        let tcp_tx_clone = audio_broadcast_tx.clone();

        tokio::spawn(async move {
            let _ = Self::run_capture_and_encode_loop(
                capture,
                capture_command_rx,
                capture_shutdown_rx,
                tcp_tx_clone,
            )
            .await;
        });

        Ok(Self {
            targets: HashSet::new(),
            audio_broadcast_tx,
            capture_command_tx,
            capture_shutdown_tx: Some(capture_shutdown_tx),
        })
    }

    async fn run_capture_and_encode_loop(
        mut capture: CaptureHandle,
        mut capture_command_rx: mpsc::Receiver<CaptureCommand>,
        mut capture_shutdown_rx: oneshot::Receiver<()>,
        audio_broadcast_tx: broadcast::Sender<Arc<Vec<u8>>>,
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

        let mut current_bitrate = Some(128_000);
        let mut encoder = create_opus_encoder().map_err(|e| AudioError::OpusInitFailed {
            direction: CodecDirection::Encoder,
            source: e,
        })?;
        let mut seq_num: u64 = 0;
        let mut frame_buf = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let mut opus_output = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet_buf: Vec<u8> = Vec::with_capacity(
            SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE,
        );
        let mut targets: HashSet<SocketAddr> = HashSet::new();
        use ringbuf::traits::*;
        let mut sample_buf = Vec::<f32>::with_capacity(OPUS_FRAME_SAMPLES * 2);

        capture.backend.play()?;

        loop {
            tokio::select! {
                Some(command) = capture_command_rx.recv() => {
                    match command {
                        CaptureCommand::AddTarget(target_addr) => {
                            let is_new = targets.insert(target_addr);
                            if is_new {
                                let _ = encoder.reset_state();
                                sample_buf.clear();
                            }
                        }
                        CaptureCommand::RemoveTarget(target_addr) => {
                            targets.remove(&target_addr);
                            if targets.is_empty() {
                                let _ = encoder.reset_state();
                                sample_buf.clear();
                            }
                        }
                        CaptureCommand::ChangeBitrate(bitrate_opt) => {
                            let previous_bitrate = current_bitrate;
                            current_bitrate = bitrate_opt;

                            if previous_bitrate != current_bitrate
                                && let Some(b) = current_bitrate
                            {
                                let _ = encoder.set_bitrate(opus::Bitrate::Bits(b));
                            }
                        }
                    }
                },
                _ = capture.notify.notified() => {
                    let mut drain_buf = [0u8; 1];
                    while audio_socket.try_recv(&mut drain_buf).is_ok() {}

                    let occupied = capture.consumer.occupied_len();
                    if occupied == 0 {
                        continue;
                    }

                    let has_tcp_listeners = audio_broadcast_tx.receiver_count() > 0;
                    if targets.is_empty() && !has_tcp_listeners {
                        while capture.consumer.try_pop().is_some() {}
                        continue;
                    }

                    let prev_len = sample_buf.len();
                    sample_buf.resize(prev_len + occupied, 0.0);

                    let actually_read = capture.consumer.pop_slice(&mut sample_buf[prev_len..]);
                    sample_buf.truncate(prev_len + actually_read);

                    while sample_buf.len() >= OPUS_FRAME_SAMPLES {
                        frame_buf.copy_from_slice(&sample_buf[..OPUS_FRAME_SAMPLES]);
                        sample_buf.drain(..OPUS_FRAME_SAMPLES);

                        let result = encode_frame(
                            &frame_buf,
                            &mut encoder,
                            current_bitrate,
                            seq_num,
                            &mut opus_output,
                            &mut packet_buf,
                        );

                        if matches!(result, EncodeResult::Skipped) {
                            continue;
                        }

                        let has_tcp_subs = audio_broadcast_tx.receiver_count() > 0;
                        let arc_packet = if has_tcp_subs {
                            let shared = Arc::new(packet_buf.clone());
                            let _ = audio_broadcast_tx.send(Arc::clone(&shared));
                            Some(shared)
                        } else {
                            None
                        };

                        for target_addr in &targets {
                            match audio_socket.try_send_to(&packet_buf, *target_addr) {
                                Ok(_) => {}
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                                Err(_) => {}
                            }
                        }

                        drop(arc_packet);
                        seq_num = seq_num.wrapping_add(1);
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
    ) -> Result<broadcast::Sender<Arc<Vec<u8>>>, GemaCastError> {
        if !self.instances.contains_key(&source) {
            if self.instances.len() >= self.max_instances {
                return Err(AudioError::CapturePoolExhausted {
                    max: self.max_instances,
                }
                .into());
            }

            let handle = match &source {
                AudioSource::Desktop => super::capture::cpal_loopback::create_cpal_loopback()?,
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
            instance.targets.insert(addr);
            let _ = instance
                .capture_command_tx
                .send(CaptureCommand::AddTarget(addr))
                .await;
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
                instance.targets.remove(&addr);
                let _ = instance
                    .capture_command_tx
                    .send(CaptureCommand::RemoveTarget(addr))
                    .await;
            }

            let is_teardown_eligible = !matches!(source, AudioSource::Desktop);
            if is_teardown_eligible
                && instance.targets.is_empty()
                && instance.audio_broadcast_tx.receiver_count() == 0
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
    ) -> Result<broadcast::Sender<Arc<Vec<u8>>>, GemaCastError> {
        self.unsubscribe(old_source, target_addr).await?;
        self.subscribe(new_source, target_addr).await
    }

    pub async fn change_bitrate(&mut self, bitrate: Option<i32>) {
        for instance in self.instances.values() {
            let _ = instance
                .capture_command_tx
                .send(CaptureCommand::ChangeBitrate(bitrate))
                .await;
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
