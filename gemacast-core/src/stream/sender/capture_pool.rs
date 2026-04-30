use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::broadcast::SenderCommand;
use super::capture::CaptureHandle;
use super::encode::{EncodeResult, encode_frame};
use crate::audio::{MAX_OPUS_PACKET_SIZE, OPUS_FRAME_SAMPLES, SEQ_NUM_SIZE, create_opus_encoder};
use crate::error::{AudioCaptureError, GemaCastError, NetworkError};
use crate::types::AudioSource;

pub struct CaptureInstance {
    pub targets: HashSet<SocketAddr>,
    pub tcp_broadcaster_tx: broadcast::Sender<Arc<Vec<u8>>>,
    pub command_tx: mpsc::Sender<SenderCommand>,
    pub stop_tx: Option<oneshot::Sender<()>>,
}

impl CaptureInstance {
    pub fn new(capture: CaptureHandle) -> Result<Self, GemaCastError> {
        let (tcp_broadcaster_tx, _) = broadcast::channel(4000);
        let (command_tx, command_rx) = mpsc::channel(32);
        let (stop_tx, stop_rx) = oneshot::channel();
        let tcp_tx_clone = tcp_broadcaster_tx.clone();

        tokio::spawn(async move {
            let _ = Self::run_encode_loop(capture, command_rx, stop_rx, tcp_tx_clone).await;
        });

        Ok(Self {
            targets: HashSet::new(),
            tcp_broadcaster_tx,
            command_tx,
            stop_tx: Some(stop_tx),
        })
    }

    async fn run_encode_loop(
        mut capture: CaptureHandle,
        mut command_rx: mpsc::Receiver<SenderCommand>,
        mut stop_rx: oneshot::Receiver<()>,
        tcp_broadcaster_tx: broadcast::Sender<Arc<Vec<u8>>>,
    ) -> Result<(), GemaCastError> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);

        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| {
            NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            }
        })?;

        let _ = socket.set_tos(0xB8);

        socket
            .bind(&addr.into())
            .map_err(|e| NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        socket
            .set_nonblocking(true)
            .map_err(|e| NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        let audio_socket =
            UdpSocket::from_std(socket.into()).map_err(|e| NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        let mut current_bitrate = Some(128_000);
        let mut encoder = create_opus_encoder().map_err(AudioCaptureError::OpusEncoderFailed)?;
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
                Some(command) = command_rx.recv() => {
                    match command {
                        SenderCommand::AddTarget(target_addr) => {
                            let is_new = targets.insert(target_addr);
                            if is_new {
                                let _ = encoder.reset_state();
                                sample_buf.clear();
                            }
                        }
                        SenderCommand::RemoveTarget(target_addr) => {
                            targets.remove(&target_addr);
                            if targets.is_empty() {
                                let _ = encoder.reset_state();
                                sample_buf.clear();
                            }
                        }
                        SenderCommand::ChangeBitrate(bitrate_opt) => {
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

                    let has_tcp_listeners = tcp_broadcaster_tx.receiver_count() > 0;
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

                        let has_tcp_subs = tcp_broadcaster_tx.receiver_count() > 0;
                        let arc_packet = if has_tcp_subs {
                            let shared = Arc::new(packet_buf.clone());
                            let _ = tcp_broadcaster_tx.send(Arc::clone(&shared));
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
                Some(stream_error) = capture.error_rx.recv() => {
                    return Err(AudioCaptureError::StreamError(stream_error).into());
                },
                _ = &mut stop_rx => {
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
    instances: HashMap<AudioSource, CaptureInstance>,
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
                return Err(AudioCaptureError::CapturePoolExhausted {
                    max: self.max_instances,
                }
                .into());
            }

            let handle = match &source {
                AudioSource::Desktop => super::capture::cpal_loopback::create_cpal_loopback()?,
                AudioSource::Process { pid, .. } => {
                    if !self.supports_process_capture {
                        return Err(AudioCaptureError::ProcessCaptureUnavailable.into());
                    }
                    #[cfg(windows)]
                    {
                        super::capture::wasapi_loopback::create_wasapi_process_loopback(*pid)?
                    }
                    #[cfg(not(windows))]
                    {
                        return Err(AudioCaptureError::ProcessCaptureUnavailable.into());
                    }
                }
            };

            let instance = CaptureInstance::new(handle)?;
            self.instances.insert(source.clone(), instance);
        }

        let instance = self.instances.get_mut(&source).unwrap();
        if let Some(addr) = target_addr {
            instance.targets.insert(addr);
            let _ = instance
                .command_tx
                .send(SenderCommand::AddTarget(addr))
                .await;
        }

        Ok(instance.tcp_broadcaster_tx.clone())
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
                    .command_tx
                    .send(SenderCommand::RemoveTarget(addr))
                    .await;
            }

            let is_teardown_eligible = !matches!(source, AudioSource::Desktop);
            if is_teardown_eligible
                && instance.targets.is_empty()
                && instance.tcp_broadcaster_tx.receiver_count() == 0
                && let Some(mut removed) = self.instances.remove(source)
                && let Some(stop_tx) = removed.stop_tx.take()
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
                .command_tx
                .send(SenderCommand::ChangeBitrate(bitrate))
                .await;
        }
    }

    pub fn available_sources(&self) -> Result<Vec<AudioSource>, GemaCastError> {
        let mut sources = vec![AudioSource::Desktop];

        // Return existing process sources so the UI knows they are active
        for source in self.instances.keys() {
            if let AudioSource::Process { .. } = source {
                sources.push(source.clone());
            }
        }

        // Ideally here we would enumerate all active audio processes,
        // but for now we just return the currently active ones + Desktop.
        // The mobile client expects to be able to request ANY PID if they have it.

        Ok(sources)
    }

    pub fn get_tcp_broadcaster(
        &self,
        source: &AudioSource,
    ) -> Option<broadcast::Sender<Arc<Vec<u8>>>> {
        self.instances
            .get(source)
            .map(|i| i.tcp_broadcaster_tx.clone())
    }
}
