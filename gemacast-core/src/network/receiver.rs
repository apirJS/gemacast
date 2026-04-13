use crate::{
    audio::{
        MAX_OPUS_PACKET_SIZE, OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE, SEQ_NUM_SIZE,
        create_opus_decoder,
    },
    error::{AudioCaptureError, GemaCastError, NetworkError},
    jitter::{JitterBufferManager, RawPacket},
    network::AUDIO_PORT,
};
use cpal::{StreamError, traits::*};
use ringbuf::{HeapProd, HeapRb, traits::*};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Instant,
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot},
};

/// Capacity of the lock-free SPSC channel carrying raw packets
/// from the network thread to the cpal audio callback.
/// 1024 slots.
const PACKET_CHANNEL_CAPACITY: usize = 1024;

pub struct AudioReceiverHandles {
    pub receiver: AudioReceiver,
    pub shutdown_tx: oneshot::Sender<()>,
    pub is_playing: Arc<AtomicBool>,
    /// Volume as f32 bits stored in a u32 (range 0.0–1.0).
    pub volume: Arc<AtomicU32>,
}

pub struct AudioReceiver {
    /// Producer side of the raw packet SPSC channel.
    /// The network thread pushes undecoded Opus packets here.
    packet_producer: HeapProd<RawPacket>,
    playback_stream: cpal::Stream,
    error_rx: mpsc::Receiver<StreamError>,
    shutdown_rx: oneshot::Receiver<()>,
    latency_metric: Arc<AtomicU32>,
}

impl AudioReceiver {
    pub async fn create() -> Result<AudioReceiverHandles, GemaCastError> {
        let (error_tx, error_rx) = mpsc::channel::<StreamError>(1);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // SPSC channel for raw Opus packets: network thread → cpal callback.
        let packet_rb = HeapRb::<RawPacket>::new(PACKET_CHANNEL_CAPACITY);
        let (packet_producer, packet_consumer) = packet_rb.split();

        // The Opus decoder lives inside the cpal callback (required for PLC to work).
        let decoder = create_opus_decoder().map_err(AudioCaptureError::OpusDecoderFailed)?;

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioCaptureError::DefaultOutputDeviceUnavailable)?;

        let mut buffer_size = cpal::BufferSize::Default;

        if let Ok(mut supported_configs) = device.supported_output_configs() {
            if let Some(config) = supported_configs.find(|c| {
                c.channels() == OPUS_CHANNELS
                    && c.min_sample_rate() <= OPUS_SAMPLE_RATE.into()
                    && c.max_sample_rate() >= OPUS_SAMPLE_RATE.into()
            }) {
                match config.buffer_size() {
                    cpal::SupportedBufferSize::Range { min, max } => {
                        let desired = OPUS_FRAME_SAMPLES as u32;
                        buffer_size = cpal::BufferSize::Fixed(desired.clamp(*min, *max));
                    }
                    cpal::SupportedBufferSize::Unknown => {}
                }
            }
        }

        let stream_config = cpal::StreamConfig {
            channels: OPUS_CHANNELS,
            sample_rate: OPUS_SAMPLE_RATE,
            buffer_size,
            // buffer_size: cpal::BufferSize::Default,
        };

        let is_playing = Arc::new(AtomicBool::new(true));
        let is_playing_for_cpal = is_playing.clone();
        let volume = Arc::new(AtomicU32::new(f32::to_bits(1.0)));
        let volume_for_cpal = volume.clone();

        let latency_metric = Arc::new(AtomicU32::new(0));
        let latency_metric_clone = latency_metric.clone();

        let playback_stream = device
            .build_output_stream(
                &stream_config,
                {
                    let mut packet_consumer = packet_consumer;
                    let mut jitter_manager =
                        JitterBufferManager::new(decoder, latency_metric_clone);

                    move |data: &mut [f32], _: &_| {
                        let vol = f32::from_bits(volume_for_cpal.load(Ordering::Relaxed));

                        if !is_playing_for_cpal.load(Ordering::Relaxed) {
                            // Paused: drain incoming packets and output silence.
                            while packet_consumer.try_pop().is_some() {}
                            for sample in data.iter_mut() {
                                *sample = 0.0;
                            }
                            jitter_manager.reset();
                            return;
                        }

                        // Step 1: Drain all raw packets from the network thread
                        //         into the jitter buffer's ordered slot array.
                        jitter_manager.ingest_packets(&mut packet_consumer);

                        // Step 2: Fill the output buffer with jitter-compensated audio.
                        //         The manager handles decode, PLC, and prebuffering
                        //         internally — the cpal DAC clock drives everything.
                        jitter_manager.fill_output(data, vol);
                    }
                },
                {
                    let error_tx = error_tx.clone();
                    move |e| {
                        let _ = error_tx.blocking_send(e);
                    }
                },
                None,
            )
            .map_err(AudioCaptureError::FailedToBuildOutputStream)?;

        let receiver = AudioReceiver {
            packet_producer,
            playback_stream,
            error_rx,
            shutdown_rx,
            latency_metric,
        };

        Ok(AudioReceiverHandles {
            receiver,
            shutdown_tx,
            is_playing,
            volume,
        })
    }

    pub async fn start_audio_listener(
        &mut self,
        sender_ip_tx: Option<oneshot::Sender<String>>,
        latency_tx: Option<mpsc::Sender<(f32, f32)>>,
        target_ip: Option<std::net::IpAddr>,
    ) -> Result<(), GemaCastError> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, AUDIO_PORT);
        let socket = UdpSocket::bind(addr)
            .await
            .map_err(|source| NetworkError::BindFailed {
                addr: addr.to_string(),
                source,
            })?;

        if let Some(target) = target_ip {
            let target_addr = std::net::SocketAddr::new(target, AUDIO_PORT);
            let _ = socket.send_to(&[0u8], target_addr).await;
            let _ = socket.send_to(&[0u8], target_addr).await;
        }

        let mut recv_buff = vec![0u8; SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE];
        let mut sender_ip_tx = sender_ip_tx;

        loop {
            tokio::select! {
                result = socket.recv_from(&mut recv_buff) => {
                    let (len, sender_addr) = match result {
                        Ok(r) => r,
                        Err(_) => continue,
                    };

                    if let Some(tx) = sender_ip_tx.take() {
                        let _ = tx.send(sender_addr.ip().to_string());
                    }

                    if len <= SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE {
                        continue;
                    }

                    let seq_bytes: [u8; 8] = recv_buff[..SEQ_NUM_SIZE].try_into().unwrap();
                    let seq_num = u64::from_be_bytes(seq_bytes);

                    let format_flag = recv_buff[SEQ_NUM_SIZE];
                    let is_uncompressed = format_flag == crate::audio::FORMAT_UNCOMPRESSED;

                    let payload_data = recv_buff[SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE..len].to_vec();

                    // Push the raw packet into the SPSC channel for the cpal callback.
                    // If the channel is full, drop the packet (real-time priority: never block).
                    let packet = RawPacket {
                        seq_num,
                        payload_data,
                        arrival_time: Instant::now(),
                        is_uncompressed,
                    };
                    let _ = self.packet_producer.try_push(packet);

                    // Latency / RMS reporting (every 50 packets, ~500ms / ~1s depending on frame size).
                    if let Some(ref tx) = latency_tx
                        && seq_num.is_multiple_of(50)
                    {
                        let sample_rate = OPUS_SAMPLE_RATE as f32;
                        let frame_samples = OPUS_FRAME_SAMPLES as f32;
                        let channels = OPUS_CHANNELS as f32;
                        let ms_per_frame = (frame_samples / channels / sample_rate) * 1000.0;

                        let rms_data = &recv_buff[SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE..len];
                        let mut rms = 0.0f32;

                        if is_uncompressed {
                            // rms_data is not guaranteed to be 4-byte aligned (offset is 9).
                            // We must safely parse chunks to avoid ARM SIGABRT.
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
                            // Packet-size heuristic: larger Opus packets carry more energy.
                            // Normalize against the typical max size (bitrate-aware).
                            let bitrate_bytes_per_sec = crate::audio::OPUS_BITRATE as f32 / 8.0;
                            let typical_max = bitrate_bytes_per_sec * ms_per_frame / 1000.0;
                            rms = (rms_data.len() as f32 / typical_max).min(1.0).sqrt();
                        }

                        let jitter_delay_ms = self.latency_metric.load(Ordering::Relaxed) as f32;
                        let buffered_packets = self.packet_producer.occupied_len();
                        let spsc_delay_ms = (buffered_packets as f32) * ms_per_frame;

                        let total_latency_ms = jitter_delay_ms + spsc_delay_ms;
                        let _ = tx.try_send((total_latency_ms, rms));
                    }
                }
                Some(stream_err) = self.error_rx.recv() => {
                    return Err(AudioCaptureError::StreamError(stream_err).into());
                }
                _ = &mut self.shutdown_rx => {
                    break;
                }
            }
        }

        Ok(())
    }

    pub fn start_audio_playback(&self) -> Result<(), GemaCastError> {
        self.playback_stream
            .play()
            .map_err(AudioCaptureError::FailedToPlayOutputStream)?;

        Ok(())
    }
}
