use crate::audio::{
    MAX_OPUS_PACKET_SIZE, OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE, SEQ_NUM_SIZE,
    create_opus_encoder,
};
use crate::error::{AudioCaptureError, GemaCastError, NetworkError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{HeapCons, HeapRb, traits::*};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc};

#[derive(Debug)]
pub enum SenderCommand {
    AddTarget(SocketAddr),
    RemoveTarget(SocketAddr),
    ChangeBitrate(Option<i32>),
}

pub struct AudioSender {
    audio_stream: cpal::Stream,
    audio_consumer: HeapCons<f32>,
    notify: Arc<Notify>,
    error_rx: mpsc::Receiver<cpal::StreamError>,
}

impl AudioSender {
    pub async fn new() -> Result<AudioSender, GemaCastError> {
        let rb = HeapRb::<f32>::new(OPUS_FRAME_SAMPLES * 64);
        let (mut rb_producer, rb_consumer) = rb.split();
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();

        let (error_tx, error_rx) = mpsc::channel::<cpal::StreamError>(1);

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioCaptureError::DefaultOutputDeviceUnavailable)?;

        let mut buffer_size = cpal::BufferSize::Default;

        let rate = OPUS_SAMPLE_RATE;
        if let Ok(mut supported_configs) = device.supported_output_configs()
            && let Some(config) = supported_configs.find(|c| {
                c.channels() == OPUS_CHANNELS
                    && c.min_sample_rate() <= rate
                    && c.max_sample_rate() >= rate
            })
            && let cpal::SupportedBufferSize::Range { min, max } = config.buffer_size() {
                let desired = OPUS_FRAME_SAMPLES as u32;
                buffer_size = cpal::BufferSize::Fixed(desired.clamp(*min, *max));
            }

        let stream_config = cpal::StreamConfig {
            channels: OPUS_CHANNELS,
            sample_rate: OPUS_SAMPLE_RATE,
            buffer_size,
        };

        let audio_stream = device
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _: &_| {
                    if rb_producer.vacant_len() >= data.len() {
                        let _ = rb_producer.push_slice(data);
                    } else {
                        eprintln!("Dropped audio frame to prevent freezing!");
                    }
                    notify_clone.notify_one();
                },
                move |e| {
                    let _ = error_tx.blocking_send(e);
                },
                None,
            )
            .map_err(AudioCaptureError::FailedToBuildInputStream)?;

        Ok(AudioSender {
            audio_consumer: rb_consumer,
            notify,
            error_rx,
            audio_stream,
        })
    }

    pub async fn start_broadcast(
        &mut self,
        mut command_rx: tokio::sync::mpsc::Receiver<SenderCommand>,
        mut stop_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), GemaCastError> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);

        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| {
            NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            }
        })?;

        let _ = socket.set_tos(0xB8); // Guarantee WMM AC_VO (DSCP 46 / EF) to bypass router policing

        // Minimize OS-level send buffering.
        // 4KB (~40 packets) is enough to absorb encoding jitter without accumulating.
        let _ = socket.set_send_buffer_size(4096);

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

        self.audio_stream
            .play()
            .map_err(AudioCaptureError::FailedToPlayInputStream)?;

        loop {
            tokio::select! {
                Some(command) = command_rx.recv() => {
                    match command {
                        SenderCommand::AddTarget(target_addr) => {
                            let is_new = targets.insert(target_addr);
                            if is_new {
                                // Ensure the Opus encoder has a clean state for any newly connected listener
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
                            current_bitrate = bitrate_opt;
                            if let Some(b) = current_bitrate {
                                let _ = encoder.set_bitrate(opus::Bitrate::Bits(b));
                            }
                        }
                    }
                },
                _ = self.notify.notified() => {
                    // Drain incoming UDP buffer (e.g. Android heartbeat ticks) so it doesn't eventually block or throw WSAEMSGSIZE
                    let mut drain_buf = [0u8; 1];
                    while audio_socket.try_recv(&mut drain_buf).is_ok() {}

                    let occupied = self.audio_consumer.occupied_len();
                    if occupied == 0 {
                        continue;
                    }

                    if targets.is_empty() {
                        while self.audio_consumer.try_pop().is_some() {}
                        continue;
                    }

                    let prev_len = sample_buf.len();
                    sample_buf.resize(prev_len + occupied, 0.0);
                    let actually_read = self.audio_consumer.pop_slice(&mut sample_buf[prev_len..]);
                    sample_buf.truncate(prev_len + actually_read);

                    while sample_buf.len() >= OPUS_FRAME_SAMPLES {
                        frame_buf.copy_from_slice(&sample_buf[..OPUS_FRAME_SAMPLES]);
                        sample_buf.drain(..OPUS_FRAME_SAMPLES);

                        // Calculate True RMS of the audio frame before encoding
                        let mut sum_sq = 0.0f32;
                        for sample in &frame_buf {
                            sum_sq += sample * sample;
                        }
                        let rms = (sum_sq / OPUS_FRAME_SAMPLES as f32).sqrt();
                        
                        let is_silence = rms < 0.0001;
                        let is_uncompressed = current_bitrate.is_none();
                        
                        let format_flag = if is_silence {
                            crate::audio::FORMAT_SILENCE
                        } else if is_uncompressed {
                            crate::audio::FORMAT_UNCOMPRESSED
                        } else {
                            crate::audio::FORMAT_OPUS
                        };

                        let payload_bytes: &[u8] = if is_silence {
                            &[] // Empty payload for perfect silence
                        } else if is_uncompressed {
                            // Safety: frame_buf is properly aligned Vec<f32>.
                            unsafe {
                                std::slice::from_raw_parts(
                                    frame_buf.as_ptr() as *const u8,
                                    OPUS_FRAME_SAMPLES * std::mem::size_of::<f32>(),
                                )
                            }
                        } else {
                            let encoded_len = match encoder.encode_float(&frame_buf, &mut opus_output) {
                                Ok(e) => e,
                                Err(e) => {
                                    eprintln!("Opus encoder failed: {}", e);
                                    continue;
                                }
                            };
                            &opus_output[..encoded_len]
                        };

                        // Reuse packet_buf — clears but does NOT deallocate.
                        packet_buf.clear();
                        packet_buf.extend_from_slice(&seq_num.to_be_bytes());
                        packet_buf.push(format_flag);
                        packet_buf.extend_from_slice(payload_bytes);

                        for target_addr in &targets {
                            match audio_socket.try_send_to(&packet_buf, *target_addr) {
                                Ok(_) => {}
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                    // OS UDP buffer full — drop this packet rather than block.
                                }
                                Err(e) => {
                                    eprintln!("UDP send failed: {}", e);
                                }
                            }
                        }

                        seq_num = seq_num.wrapping_add(1);
                    }
                },
                Some(stream_error) = self.error_rx.recv() => {
                    return Err(AudioCaptureError::StreamError(stream_error).into());
                },
                _ = &mut stop_rx => {
                    let _ = self.audio_stream.pause();
                    break;
                }
                else => break,
            }
        }

        Ok(())
    }
}
