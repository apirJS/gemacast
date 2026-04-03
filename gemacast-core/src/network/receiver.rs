use crate::{
    audio::{
        MAX_OPUS_PACKET_SIZE, OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_FRAME_SIZE, OPUS_SAMPLE_RATE,
        SEQ_NUM_SIZE, create_opus_decoder,
    },
    error::{AudioCaptureError, GemaCastError, NetworkError},
    network::AUDIO_PORT,
};
use cpal::{StreamError, traits::*};
use ringbuf::{HeapProd, HeapRb, traits::*};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::{Arc, atomic::AtomicBool},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot},
};

pub struct AudioReceiverHandles {
    pub receiver: AudioReceiver,
    pub shutdown_tx: oneshot::Sender<()>,
    pub is_playing: Arc<AtomicBool>,
}

pub struct AudioReceiver {
    rb_producer: HeapProd<f32>,
    playback_stream: cpal::Stream,
    error_rx: mpsc::Receiver<StreamError>,
    shutdown_rx: oneshot::Receiver<()>,
}

impl AudioReceiver {
    pub async fn create() -> Result<AudioReceiverHandles, GemaCastError> {
        let (error_tx, error_rx) = mpsc::channel::<StreamError>(1);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let rb = HeapRb::<f32>::new(OPUS_FRAME_SAMPLES * 16);
        let (rb_producer, rb_consumer) = rb.split();

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| AudioCaptureError::DefaultOutputDeviceUnavailable)?;

        let stream_config = cpal::StreamConfig {
            channels: OPUS_CHANNELS,
            sample_rate: OPUS_SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Fixed(OPUS_FRAME_SIZE as u32),
        };

        let is_playing = Arc::new(AtomicBool::new(false));
        let is_playing_for_cpal = is_playing.clone();

        let playback_stream = device
            .build_output_stream(
                &stream_config,
                {
                    let mut rb_consumer = rb_consumer;
                    let mut prebuffering = true;
                    let target_cushion = OPUS_FRAME_SAMPLES * 6;

                    move |data: &mut [f32], _: &_| {
                        if !is_playing_for_cpal.load(std::sync::atomic::Ordering::Relaxed) {
                            while rb_consumer.try_pop().is_some() {}

                            for sample in data.iter_mut() {
                                *sample = 0.0;
                            }
                            prebuffering = true;
                            return;
                        }

                        if prebuffering {
                            if rb_consumer.occupied_len() >= target_cushion {
                                prebuffering = false;
                            } else {
                                for sample in data.iter_mut() {
                                    *sample = 0.0;
                                }
                                return;
                            }
                        }

                        let mut underrun = false;
                        for sample in data.iter_mut() {
                            if let Some(s) = rb_consumer.try_pop() {
                                *sample = s;
                            } else {
                                *sample = 0.0;
                                underrun = true;
                            }
                        }

                        if underrun {
                            prebuffering = true;
                        }
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
            .map_err(|e| AudioCaptureError::FailedToBuildOutputStream(e))?;

        let receiver = AudioReceiver {
            rb_producer,
            playback_stream,
            error_rx,
            shutdown_rx,
        };

        Ok(AudioReceiverHandles {
            receiver,
            shutdown_tx,
            is_playing,
        })
    }

    pub async fn start_audio_listener(
        &mut self,
        sender_ip_tx: Option<oneshot::Sender<String>>,
        latency_tx: Option<mpsc::Sender<f32>>,
    ) -> Result<(), GemaCastError> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, AUDIO_PORT);
        let socket = UdpSocket::bind(addr)
            .await
            .map_err(|source| NetworkError::BindFailed {
                addr: addr.to_string(),
                source,
            })?;

        let mut highest_seq_num: u64 = 0;
        let mut decoder =
            create_opus_decoder().map_err(|e| AudioCaptureError::OpusDecoderFailed(e))?;
        let mut recv_buff = vec![0u8; SEQ_NUM_SIZE + MAX_OPUS_PACKET_SIZE];
        let mut pcm_output = vec![0f32; OPUS_FRAME_SAMPLES];
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

                    if len <= SEQ_NUM_SIZE {
                        continue;
                    }

                    let seq_bytes: [u8; 8] = recv_buff[..SEQ_NUM_SIZE].try_into().unwrap();
                    let seq_num = u64::from_be_bytes(seq_bytes);

                    if seq_num <= highest_seq_num && highest_seq_num != 0 {
                        continue;
                    }

                    // --- Packet Loss Concealment ---
                    if highest_seq_num != 0 && seq_num > highest_seq_num + 1 {
                        let missing_frames = (seq_num - highest_seq_num - 1).min(3);
                        for _ in 0..missing_frames {
                            if let Ok(samples) = decoder.decode_float(&[] as &[u8], &mut pcm_output, false) {
                                let total = samples * OPUS_CHANNELS as usize;
                                if self.rb_producer.vacant_len() >= total {
                                    self.rb_producer.push_slice(&pcm_output[..total]);
                                }
                            }
                        }
                    }

                    highest_seq_num = seq_num;

                    let opus_data = &recv_buff[SEQ_NUM_SIZE..len];
                    let decoded_samples = match decoder.decode_float(opus_data, &mut pcm_output, false) {
                        Ok(len) => len,
                        Err(_) => continue,
                    };

                    let total_samples = decoded_samples * OPUS_CHANNELS as usize;
                    self.rb_producer.push_slice(&pcm_output[..total_samples]);

                    if let Some(ref tx) = latency_tx {
                        if highest_seq_num % 50 == 0 {
                            let latency_ms = (self.rb_producer.occupied_len() as f32
                                / OPUS_CHANNELS as f32
                                / OPUS_SAMPLE_RATE as f32)
                                * 1000.0;
                            let _ = tx.try_send(latency_ms);
                        }
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
            .map_err(|e| AudioCaptureError::FailedToPlayOutputStream(e))?;

        Ok(())
    }
}
