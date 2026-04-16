use crate::{
    audio::{
        MAX_OPUS_PACKET_SIZE, OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE, SEQ_NUM_SIZE,
        create_opus_decoder,
    },
    error::{AudioCaptureError, GemaCastError, NetworkError},
    jitter::{JitterBufferManager, RawPacket},
    network::AUDIO_PORT,
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

#[cfg(not(target_os = "android"))]
pub type PlaybackStream = cpal::Stream;

#[cfg(target_os = "android")]
pub type PlaybackStream = oboe::AudioStreamAsync<oboe::Output, OboeCallback>;

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

        // Safely cast &mut [(f32, f32)] to &mut [f32] for the jitter manager
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
    pub async fn create() -> Result<AudioReceiverHandles, GemaCastError> {
        #[allow(unused_variables)]
        let (error_tx, error_rx) = mpsc::channel::<StreamError>(1);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // SPSC channel for raw Opus packets: network thread → cpal callback.
        let packet_rb = HeapRb::<RawPacket>::new(PACKET_CHANNEL_CAPACITY);
        let (packet_producer, packet_consumer) = packet_rb.split();

        // The Opus decoder lives inside the cpal callback (required for PLC to work).
        let decoder = create_opus_decoder().map_err(AudioCaptureError::OpusDecoderFailed)?;

        let _host = cpal::default_host();
        let is_playing = Arc::new(AtomicBool::new(true));
        let is_playing_for_cpal = is_playing.clone();
        let volume = Arc::new(AtomicU32::new(f32::to_bits(1.0)));
        let volume_for_cpal = volume.clone();

        let latency_metric = Arc::new(AtomicU32::new(0));
        let latency_metric_clone = latency_metric.clone();

        #[cfg(not(target_os = "android"))]
        let playback_stream = {
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

            device
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

                            jitter_manager.ingest_packets(&mut packet_consumer);
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
                .map_err(AudioCaptureError::FailedToBuildOutputStream)?
        };

        #[cfg(target_os = "android")]
        let playback_stream = {
            let callback = OboeCallback {
                jitter_manager: JitterBufferManager::new(decoder, latency_metric_clone),
                packet_consumer,
                volume: volume_for_cpal,
                is_playing: is_playing_for_cpal,
            };

            let builder = AudioStreamBuilder::default()
                .set_direction::<oboe::Output>()
                .set_performance_mode(PerformanceMode::LowLatency)
                .set_sharing_mode(SharingMode::Shared)
                .set_format::<f32>()
                .set_channel_count::<oboe::Stereo>()
                .set_sample_rate(OPUS_SAMPLE_RATE as i32)
                .set_callback(callback);

            let stream = builder.open_stream().map_err(|_| {
                AudioCaptureError::FailedToBuildOutputStream(
                    cpal::BuildStreamError::DeviceNotAvailable,
                )
            })?;
            stream
        };

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

        let mut recv_buff =
            vec![0u8; SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE];
        let mut sender_ip_tx = sender_ip_tx;

        let heartbeat_active = Arc::new(AtomicBool::new(true));
        let hb_active_clone = heartbeat_active.clone();

        let mut heartbeat_thread = target_ip.map(|target| {
            let target_addr = std::net::SocketAddr::new(target, AUDIO_PORT);
            std::thread::spawn(move || {
                #[cfg(target_os = "android")]
                unsafe {
                    // Elevate to THREAD_PRIORITY_URGENT_AUDIO (-19) to bypass MIUI aggressive CPU throttling.
                    // This physically guarantees the 20ms sleep is honored, forcing Wi-Fi awake.
                    libc::setpriority(libc::PRIO_PROCESS, 0, -19);
                }
                
                if let Ok(hb_socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
                    while hb_active_clone.load(Ordering::Relaxed) {
                        let _ = hb_socket.send_to(&[0u8], target_addr);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            })
        });

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
                        && seq_num.is_multiple_of(100)
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
                        // jitter_delay_ms holds the EXACT time elapsed since the packet hit the network socket
                        // until it was played. This ALREADY includes SPSC queue time!
                        let total_latency_ms = jitter_delay_ms;
                        let _ = tx.try_send((total_latency_ms, rms));

                        eprintln!(
                            "GemaCastLatencyLog | Seq: {}, Latency: {}ms, RMS: {:.4}",
                            seq_num, total_latency_ms, rms
                        );
                    }
                }
                Some(stream_err) = self.error_rx.recv() => {
                    heartbeat_active.store(false, Ordering::Relaxed);
                    if let Some(handle) = heartbeat_thread.take() {
                        let _ = handle.join();
                    }
                    return Err(AudioCaptureError::StreamError(stream_err).into());
                }
                _ = &mut self.shutdown_rx => {
                    heartbeat_active.store(false, Ordering::Relaxed);
                    if let Some(handle) = heartbeat_thread.take() {
                        let _ = handle.join();
                    }
                    break;
                }
            }
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

            // Crucial Android Low-Latency tuning: force the hardware mixer buffer down
            // to a double-buffer of its smallest supported burst size.
            let burst = self.playback_stream.get_frames_per_burst();
            let _ = self.playback_stream.set_buffer_size_in_frames(burst * 2);
        }
        Ok(())
    }
}
