use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use crate::error::GemaCastError;
use crate::types::{AudioSource, DeviceId};
use tokio::sync::watch;

use super::capture_pool::CapturePool;

#[derive(Debug)]
pub enum CaptureCommand {
    AddTarget(SocketAddr),
    RemoveTarget(SocketAddr),
    ChangeBitrate(Option<i32>),
}

pub struct AudioStreamEngine {
    pub pool: CapturePool,
    pub active_receiver_sessions: HashMap<DeviceId, (Option<SocketAddr>, AudioSource)>,
    tcp_source_watch_tx: watch::Sender<Option<broadcast::Sender<Arc<Vec<u8>>>>>,
    tcp_source_watch_rx: watch::Receiver<Option<broadcast::Sender<Arc<Vec<u8>>>>>,
}

pub enum AudioStreamCommand {
    Subscribe {
        device_id: DeviceId,
        target_addr: Option<SocketAddr>,
        source: AudioSource,
    },
    Unsubscribe {
        device_id: DeviceId,
    },
    ChangeSource {
        device_id: DeviceId,
        source: AudioSource,
    },
    ChangeBitrate(Option<i32>),
    Shutdown,
}

impl AudioStreamEngine {
    pub fn new(supports_process_capture: bool) -> Self {
        let (tcp_source_watch_tx, tcp_source_watch_rx) = watch::channel(None);
        Self {
            pool: CapturePool::new(supports_process_capture),
            active_receiver_sessions: HashMap::new(),
            tcp_source_watch_tx,
            tcp_source_watch_rx,
        }
    }

    pub fn tcp_source_watch(&self) -> watch::Receiver<Option<broadcast::Sender<Arc<Vec<u8>>>>> {
        self.tcp_source_watch_rx.clone()
    }

    pub fn seed_tcp_source(&self, broadcast_tx: broadcast::Sender<Arc<Vec<u8>>>) {
        let _ = self.tcp_source_watch_tx.send(Some(broadcast_tx));
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
                } => {
                    tracing::info!(
                        "[Engine] Subscribe device={:?} source={:?} target_addr={:?}",
                        device_id,
                        source,
                        target_addr
                    );
                    match self.pool.subscribe(source.clone(), target_addr).await {
                        Ok(broadcast_tx) => {
                            if target_addr.is_none() {
                                tracing::info!(
                                    "[Engine] Sending initial broadcast to ADB TCP watch"
                                );
                                let _ = self.tcp_source_watch_tx.send(Some(broadcast_tx));
                            }
                            self.active_receiver_sessions
                                .insert(device_id, (target_addr, source));
                        }
                        Err(e) => {
                            tracing::error!("[Engine] Subscribe failed: {}", e);
                            continue;
                        }
                    }
                }
                AudioStreamCommand::Unsubscribe { device_id } => {
                    tracing::info!("[Engine] Unsubscribe device={:?}", device_id);
                    if let Some((target_addr, source)) =
                        self.active_receiver_sessions.remove(&device_id)
                    {
                        let _ = self.pool.unsubscribe(&source, target_addr).await;
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
                    if let Some((target_addr, old_source)) =
                        self.active_receiver_sessions.get(&device_id)
                    {
                        let old_source = old_source.clone();
                        let target_addr = *target_addr;
                        tracing::info!(
                            "[Engine] Found session: old_source={:?} target_addr={:?}",
                            old_source,
                            target_addr
                        );

                        match self
                            .pool
                            .change_source(&old_source, source.clone(), target_addr)
                            .await
                        {
                            Ok(broadcast_tx) => {
                                tracing::info!("[Engine] Source changed successfully");
                                if target_addr.is_none() {
                                    tracing::info!(
                                        "[Engine] Sending new broadcast to ADB TCP watch"
                                    );
                                    let _ = self.tcp_source_watch_tx.send(Some(broadcast_tx));
                                }
                                self.active_receiver_sessions
                                    .insert(device_id, (target_addr, source));
                            }
                            Err(e) => {
                                let err_msg = match e {
                                    #[cfg(target_os = "windows")]
                                    crate::error::GemaCastError::Audio(
                                        crate::error::AudioError::WindowsApi(we),
                                    ) => {
                                        format!("Windows API error: {:#010x}", we.code().0)
                                    }
                                    _ => String::from("Non-Windows API error"),
                                };
                                tracing::error!(
                                    "[Engine] Failed to change source from {:?} to {:?}: {}",
                                    old_source,
                                    source,
                                    err_msg
                                );
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[Engine] ChangeSource: device {:?} not found in active sessions",
                            device_id
                        );
                    }
                }
                AudioStreamCommand::ChangeBitrate(bitrate_opt) => {
                    self.pool.change_bitrate(bitrate_opt).await;
                }
                AudioStreamCommand::Shutdown => {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn get_tcp_broadcaster(
        &self,
        source: &AudioSource,
    ) -> Option<broadcast::Sender<Arc<Vec<u8>>>> {
        self.pool.get_tcp_broadcaster(source)
    }
}
