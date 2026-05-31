use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};

use crate::control::http::send_ws_event;
use crate::control::types::WsEvent;
use crate::error::GemaCastError;
use crate::types::{AudioSource, DeviceId, TargetId};

use super::capture::DefaultCaptureFactory;
use super::capture_pool::CapturePool;

#[derive(Debug)]
pub enum CaptureCommand {
    AddTarget {
        addr: SocketAddr,
        bitrate: Option<i32>,
    },
    RemoveTarget(SocketAddr),
}

pub type WsConnectionMap = Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>;

pub struct AudioStreamEngine {
    pub pool: CapturePool,
    pub active_receiver_sessions: HashMap<DeviceId, (Option<SocketAddr>, AudioSource, Option<i32>)>,
    ws_connections: WsConnectionMap,
}

pub enum AudioStreamCommand {
    Subscribe {
        device_id: DeviceId,
        target_addr: Option<SocketAddr>,
        source: Option<AudioSource>,
        bitrate: Option<i32>,
    },
    Unsubscribe {
        device_id: DeviceId,
    },
    ChangeSource {
        device_id: DeviceId,
        source: AudioSource,
    },
    ChangeBitrate {
        device_id: DeviceId,
        bitrate: Option<i32>,
    },
    GetTcpBroadcaster {
        device_id: DeviceId,
        reply: tokio::sync::oneshot::Sender<Option<broadcast::Sender<Arc<Vec<u8>>>>>,
    },
    Shutdown,
}

impl AudioStreamEngine {
    pub fn new(supports_process_capture: bool, ws_connections: WsConnectionMap) -> Self {
        Self::new_with_factory(
            Box::new(DefaultCaptureFactory),
            supports_process_capture,
            ws_connections,
        )
    }

    pub fn new_with_factory(
        factory: Box<dyn crate::stream::sender::capture::CaptureFactory>,
        supports_process_capture: bool,
        ws_connections: WsConnectionMap,
    ) -> Self {
        Self {
            pool: CapturePool::new(factory, supports_process_capture),
            active_receiver_sessions: HashMap::new(),
            ws_connections,
        }
    }

    pub async fn run_command_loop(
        &mut self,
        mut audio_engine_command_rx: mpsc::Receiver<AudioStreamCommand>,
    ) -> Result<(), GemaCastError> {
        while let Some(command) = audio_engine_command_rx.recv().await {
            match command {
                AudioStreamCommand::Subscribe {
                    device_id,
                    target_addr,
                    source,
                    bitrate,
                } => {
                    let mut final_source = source.unwrap_or_default();
                    if let Some((_, existing_source, _)) =
                        self.active_receiver_sessions.get(&device_id)
                    {
                        final_source = existing_source.clone();
                    }

                    tracing::info!(
                        "[Engine] Subscribe device={:?} source={:?} target_addr={:?} bitrate={:?}",
                        device_id,
                        final_source,
                        target_addr,
                        bitrate
                    );

                    let target = if let Some(addr) = target_addr {
                        TargetId::Udp(addr)
                    } else {
                        TargetId::Tcp(device_id.clone())
                    };

                    // If device is already subscribed (e.g. fast reconnect), we might want to clean up first
                    if self.active_receiver_sessions.contains_key(&device_id) {
                        tracing::info!(
                            "[Engine] Cleaning up existing session for device={:?}",
                            device_id
                        );
                        let _ = self.pool.unsubscribe(&final_source, target.clone()).await;
                    }

                    match self
                        .pool
                        .subscribe(final_source.clone(), target, bitrate)
                        .await
                    {
                        Ok(_) => {
                            self.active_receiver_sessions
                                .insert(device_id, (target_addr, final_source, bitrate));
                        }
                        Err(e) => {
                            let msg = format!("Audio capture failed: {}", e);
                            tracing::error!("[Engine] Subscribe failed: {}", msg);
                            let _ = send_ws_event(
                                &self.ws_connections,
                                &device_id,
                                WsEvent::Error { message: msg },
                            )
                            .await;
                        }
                    }
                }
                AudioStreamCommand::Unsubscribe { device_id } => {
                    tracing::info!("[Engine] Unsubscribe device={:?}", device_id);
                    if let Some((target_addr, source, _bitrate)) =
                        self.active_receiver_sessions.remove(&device_id)
                    {
                        let target = if let Some(addr) = target_addr {
                            TargetId::Udp(addr)
                        } else {
                            TargetId::Tcp(device_id)
                        };
                        let _ = self.pool.unsubscribe(&source, target).await;
                    }
                }
                AudioStreamCommand::ChangeSource { device_id, source } => {
                    tracing::info!(
                        "[Engine] ChangeSource device={:?} new_source={:?}",
                        device_id,
                        source
                    );

                    tracing::info!(
                        "[Engine] Active sessions: {:?}",
                        self.active_receiver_sessions.keys().collect::<Vec<_>>()
                    );

                    if let Some((target_addr, old_source, bitrate)) =
                        self.active_receiver_sessions.get(&device_id)
                    {
                        let old_source = old_source.clone();
                        let target_addr = *target_addr;
                        let bitrate = *bitrate;
                        tracing::info!(
                            "[Engine] Found session: old_source={:?} target_addr={:?}",
                            old_source,
                            target_addr
                        );

                        let target = if let Some(addr) = target_addr {
                            TargetId::Udp(addr)
                        } else {
                            TargetId::Tcp(device_id.clone())
                        };

                        match self
                            .pool
                            .change_source(&old_source, source.clone(), target, bitrate)
                            .await
                        {
                            Ok(_broadcast_tx) => {
                                tracing::info!("[Engine] Source changed successfully");
                                self.active_receiver_sessions
                                    .insert(device_id, (target_addr, source, bitrate));
                            }
                            Err(e) => {
                                let msg = format!("Failed to change audio source: {}", e);
                                tracing::error!(
                                    "[Engine] Failed to change source from {:?} to {:?}: {}",
                                    old_source,
                                    source,
                                    msg
                                );
                                let _ = send_ws_event(
                                    &self.ws_connections,
                                    &device_id,
                                    WsEvent::Error { message: msg },
                                )
                                .await;
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[Engine] ChangeSource: device {:?} not found in active sessions",
                            device_id
                        );
                    }
                }
                AudioStreamCommand::ChangeBitrate { device_id, bitrate } => {
                    tracing::info!(
                        "[Engine] ChangeBitrate device={:?} new_bitrate={:?}",
                        device_id,
                        bitrate
                    );

                    if let Some((target_addr, source, old_bitrate)) =
                        self.active_receiver_sessions.get(&device_id)
                    {
                        if *old_bitrate == bitrate {
                            tracing::info!("[Engine] Bitrate unchanged, skipping.");
                            continue;
                        }

                        let source_clone = source.clone();
                        let target_addr_clone = *target_addr;

                        tracing::info!(
                            "[Engine] Found session to update bitrate: source={:?} target_addr={:?}",
                            source_clone,
                            target_addr_clone
                        );

                        let target = if let Some(addr) = target_addr_clone {
                            TargetId::Udp(addr)
                        } else {
                            TargetId::Tcp(device_id.clone())
                        };

                        match self
                            .pool
                            .change_bitrate(&source_clone, target, bitrate)
                            .await
                        {
                            Ok(_broadcast_tx) => {
                                tracing::info!("[Engine] Bitrate changed successfully");
                                self.active_receiver_sessions
                                    .insert(device_id, (target_addr_clone, source_clone, bitrate));
                            }
                            Err(e) => {
                                let msg = format!("Failed to change bitrate: {}", e);
                                tracing::error!("[Engine] Bitrate change failed: {}", msg);
                                let _ = send_ws_event(
                                    &self.ws_connections,
                                    &device_id,
                                    WsEvent::Error { message: msg },
                                )
                                .await;
                            }
                        }
                    }
                }
                AudioStreamCommand::GetTcpBroadcaster { device_id, reply } => {
                    tracing::info!("[Engine] GetTcpBroadcaster for device={:?}", device_id);
                    if let Some((target_addr, source, bitrate)) =
                        self.active_receiver_sessions.get(&device_id)
                    {
                        let target = if let Some(addr) = target_addr {
                            TargetId::Udp(*addr)
                        } else {
                            TargetId::Tcp(device_id.clone())
                        };

                        match self.pool.subscribe(source.clone(), target, *bitrate).await {
                            Ok(Some(broadcast_tx)) => {
                                let _ = reply.send(Some(broadcast_tx));
                            }
                            Ok(None) => {
                                tracing::warn!(
                                    "[Engine] GetTcpBroadcaster: Expected broadcast_tx for TCP target, got None (device={:?})",
                                    device_id
                                );
                                let _ = reply.send(None);
                            }
                            Err(e) => {
                                tracing::error!(
                                    "[Engine] Failed to get broadcaster for device={:?}: {}",
                                    device_id,
                                    e
                                );
                                let _ = reply.send(None);
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[Engine] GetTcpBroadcaster: No active session for device={:?}",
                            device_id
                        );
                        let _ = reply.send(None);
                    }
                }
                AudioStreamCommand::Shutdown => {
                    tracing::info!("[Engine] Shutdown");
                    break;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::sender::capture::{CaptureBackend, CaptureFactory, CaptureHandle};
    use ringbuf::HeapRb;
    use ringbuf::traits::*;
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
            self.create_desktop_capture()
        }
    }

    #[tokio::test]
    async fn should_register_session_on_subscribe() {
        let ws_connections = Arc::new(Mutex::new(HashMap::new()));
        let mut engine =
            AudioStreamEngine::new_with_factory(Box::new(MockCaptureFactory), true, ws_connections);
        let (tx, rx) = mpsc::channel(10);
        let device_id = DeviceId("test-device".to_string());

        let target_addr = Some("127.0.0.1:1234".parse().unwrap());
        let source = AudioSource::Desktop;

        tx.send(AudioStreamCommand::Subscribe {
            device_id: device_id.clone(),
            target_addr,
            source: Some(source.clone()),
            bitrate: None,
        })
        .await
        .unwrap();

        tx.send(AudioStreamCommand::Shutdown).await.unwrap();

        engine.run_command_loop(rx).await.unwrap();

        assert!(engine.active_receiver_sessions.contains_key(&device_id));
        let (actual_target, actual_source, _) =
            engine.active_receiver_sessions.get(&device_id).unwrap();
        assert_eq!(*actual_target, target_addr);
        assert_eq!(*actual_source, source);
    }

    #[tokio::test]
    async fn should_update_session_source_on_change_source() {
        let ws_connections = Arc::new(Mutex::new(HashMap::new()));
        let mut engine =
            AudioStreamEngine::new_with_factory(Box::new(MockCaptureFactory), true, ws_connections);
        let (tx, rx) = mpsc::channel(10);
        let device_id = DeviceId("test-device-2".to_string());

        tx.send(AudioStreamCommand::Subscribe {
            device_id: device_id.clone(),
            target_addr: None, // TCP mode
            source: Some(AudioSource::Desktop),
            bitrate: None,
        })
        .await
        .unwrap();

        tx.send(AudioStreamCommand::ChangeSource {
            device_id: device_id.clone(),
            source: AudioSource::Process {
                pid: 1234,
                name: "test".to_string(),
            },
        })
        .await
        .unwrap();

        tx.send(AudioStreamCommand::Shutdown).await.unwrap();

        engine.run_command_loop(rx).await.unwrap();

        assert!(engine.active_receiver_sessions.contains_key(&device_id));
        let (_, actual_source, _) = engine.active_receiver_sessions.get(&device_id).unwrap();
        assert_eq!(
            *actual_source,
            AudioSource::Process {
                pid: 1234,
                name: "test".to_string()
            }
        );
    }

    #[tokio::test]
    async fn should_remove_session_on_unsubscribe() {
        let ws_connections = Arc::new(Mutex::new(HashMap::new()));
        let mut engine =
            AudioStreamEngine::new_with_factory(Box::new(MockCaptureFactory), true, ws_connections);
        let (tx, rx) = mpsc::channel(10);
        let device_id = DeviceId("test-device-3".to_string());

        tx.send(AudioStreamCommand::Subscribe {
            device_id: device_id.clone(),
            target_addr: None,
            source: Some(AudioSource::Desktop),
            bitrate: None,
        })
        .await
        .unwrap();

        tx.send(AudioStreamCommand::Unsubscribe {
            device_id: device_id.clone(),
        })
        .await
        .unwrap();

        tx.send(AudioStreamCommand::Shutdown).await.unwrap();

        engine.run_command_loop(rx).await.unwrap();

        assert!(!engine.active_receiver_sessions.contains_key(&device_id));
    }
}
