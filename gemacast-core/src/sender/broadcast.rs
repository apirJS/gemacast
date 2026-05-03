use crate::audio::{MAX_OPUS_PACKET_SIZE, OPUS_FRAME_SAMPLES, SEQ_NUM_SIZE, create_opus_encoder};
use crate::error::{AudioCaptureError, GemaCastError, NetworkError};
use ringbuf::traits::*;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use super::capture::{CaptureHandle, create_cpal_loopback};
use super::encode::{EncodeResult, encode_frame};

#[derive(Debug)]
pub enum SenderCommand {
    AddTarget(SocketAddr),
    RemoveTarget(SocketAddr),
    ChangeBitrate(Option<i32>),
}

pub struct AudioSender {
    capture: CaptureHandle,
    pub tcp_broadcaster_tx: tokio::sync::broadcast::Sender<Arc<Vec<u8>>>,
}

impl AudioSender {
    pub async fn new() -> Result<Self, GemaCastError> {
        let capture = create_cpal_loopback()?;
        Self::with_capture(capture)
    }

    pub fn with_capture(capture: CaptureHandle) -> Result<Self, GemaCastError> {
        let (tcp_broadcaster_tx, _) = tokio::sync::broadcast::channel(4000);
        Ok(Self {
            capture,
            tcp_broadcaster_tx,
        })
    }

    pub async fn start_broadcast(
        &mut self,
        mut command_rx: mpsc::Receiver<SenderCommand>,
        mut stop_rx: tokio::sync::oneshot::Receiver<()>,
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
        let mut sample_buf = Vec::<f32>::with_capacity(OPUS_FRAME_SAMPLES * 2);

        self.capture.backend.play()?;

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
                _ = self.capture.notify.notified() => {
                    let mut drain_buf = [0u8; 1];
                    while audio_socket.try_recv(&mut drain_buf).is_ok() {}

                    let occupied = self.capture.consumer.occupied_len();
                    if occupied == 0 {
                        continue;
                    }

                    let has_tcp_listeners = self.tcp_broadcaster_tx.receiver_count() > 0;
                    if targets.is_empty() && !has_tcp_listeners {
                        while self.capture.consumer.try_pop().is_some() {}
                        continue;
                    }

                    let prev_len = sample_buf.len();
                    sample_buf.resize(prev_len + occupied, 0.0);
                    let actually_read = self.capture.consumer.pop_slice(&mut sample_buf[prev_len..]);
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

                        let has_tcp_subs = self.tcp_broadcaster_tx.receiver_count() > 0;
                        let arc_packet = if has_tcp_subs {
                            let shared = Arc::new(packet_buf.clone());
                            let _ = self.tcp_broadcaster_tx.send(Arc::clone(&shared));
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
                Some(stream_error) = self.capture.error_rx.recv() => {
                    return Err(AudioCaptureError::StreamError(stream_error).into());
                },
                _ = &mut stop_rx => {
                    let _ = self.capture.backend.pause();
                    break;
                }
                else => break,
            }
        }

        Ok(())
    }
}
