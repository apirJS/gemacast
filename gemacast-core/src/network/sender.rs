use crate::audio::{
    FrameAccumulator, MAX_OPUS_PACKET_SIZE, OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE,
    SEQ_NUM_SIZE, create_opus_encoder,
};
use crate::error::{AudioCaptureError, GemaCastError, NetworkError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{HeapCons, HeapRb, traits::*};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc};

#[derive(Debug)]
pub enum SenderCommand {
    AddTarget(SocketAddr),
    RemoveTarget(SocketAddr),
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

        let stream_config = cpal::StreamConfig {
            channels: OPUS_CHANNELS,
            sample_rate: OPUS_SAMPLE_RATE,
            // buffer_size: cpal::BufferSize::Fixed(64),
            buffer_size: cpal::BufferSize::Default,
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
        let audio_socket = UdpSocket::bind(addr)
            .await
            .map_err(|e| NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            })?;
        let mut encoder =
            create_opus_encoder().map_err(|e| AudioCaptureError::OpusEncoderFailed(e))?;
        let mut frame_accumulator = FrameAccumulator::new(OPUS_FRAME_SAMPLES);
        let mut seq_num: u64 = 0;
        let mut opus_output = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut targets: HashSet<SocketAddr> = HashSet::new();

        let _ = self
            .audio_stream
            .play()
            .map_err(|e| AudioCaptureError::FailedToPlayInputStream(e))?;

        loop {
            tokio::select! {
                Some(command) = command_rx.recv() => {
                    match command {
                        SenderCommand::AddTarget(target_addr) => {
                            targets.insert(target_addr);
                        }
                        SenderCommand::RemoveTarget(target_addr) => {
                            targets.remove(&target_addr);
                        }
                    }
                },
                _ = self.notify.notified() => {
                    let occupied = self.audio_consumer.occupied_len();
                    if occupied == 0 {
                        continue;
                    }

                    if targets.is_empty() {
                        while self.audio_consumer.try_pop().is_some() {}
                        continue;
                    }

                    let mut samples = Vec::with_capacity(occupied);
                    while let Some(s) = self.audio_consumer.try_pop() {
                        samples.push(s);
                    }

                    let frames = frame_accumulator.push(&samples);
                    for frame in frames {
                        let encoded_len = match encoder.encode_float(&frame, &mut opus_output) {
                            Ok(e) => e,
                            Err(e) => {
                                eprintln!("Opus encoder failed: {}", e);
                                continue;
                            }
                        };

                        let mut packet = Vec::with_capacity(SEQ_NUM_SIZE + encoded_len);

                        packet.extend_from_slice(&seq_num.to_be_bytes());
                        packet.extend_from_slice(&opus_output[..encoded_len]);

                        for target_addr in &targets {
                            if let Err(e) = audio_socket.send_to(&packet, *target_addr).await {
                                eprintln!("UDP send failed: {}", e);
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
