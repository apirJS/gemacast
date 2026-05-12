use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use crate::error::GemaCastError;
use crate::types::{AudioSource, DeviceId};

use super::capture_pool::CapturePool;

#[derive(Debug)]
pub enum CaptureCommand {
    AddTarget {
        addr: SocketAddr,
        bitrate: Option<i32>,
    },
    RemoveTarget(SocketAddr),
}

pub struct AudioStreamEngine {
    pub pool: CapturePool,
    /// Maps device_id → (target_addr, source, bitrate)
    pub active_receiver_sessions: HashMap<DeviceId, (Option<SocketAddr>, AudioSource, Option<i32>)>,
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
    pub fn new(supports_process_capture: bool) -> Self {
        Self {
            pool: CapturePool::new(supports_process_capture),
            active_receiver_sessions: HashMap::new(),
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

                    // If device is already subscribed (e.g. fast reconnect), we might want to clean up first
                    if self.active_receiver_sessions.contains_key(&device_id) {
                        tracing::info!(
                            "[Engine] Cleaning up existing session for device={:?}",
                            device_id
                        );
                        let _ = self.pool.unsubscribe(&final_source, target_addr).await;
                    }

                    match self
                        .pool
                        .subscribe(final_source.clone(), target_addr, bitrate)
                        .await
                    {
                        Ok(_) => {
                            self.active_receiver_sessions
                                .insert(device_id, (target_addr, final_source, bitrate));
                        }
                        Err(e) => {
                            tracing::error!("[Engine] Subscribe failed: {}", e);
                        }
                    }
                }
                AudioStreamCommand::Unsubscribe { device_id } => {
                    tracing::info!("[Engine] Unsubscribe device={:?}", device_id);
                    if let Some((target_addr, source, _bitrate)) =
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

                        match self
                            .pool
                            .change_source(&old_source, source.clone(), target_addr, bitrate)
                            .await
                        {
                            Ok(_broadcast_tx) => {
                                tracing::info!("[Engine] Source changed successfully");
                                self.active_receiver_sessions
                                    .insert(device_id, (target_addr, source, bitrate));
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

                        match self
                            .pool
                            .change_bitrate(&source_clone, target_addr_clone, bitrate)
                            .await
                        {
                            Ok(_broadcast_tx) => {
                                tracing::info!("[Engine] Bitrate changed successfully");
                                self.active_receiver_sessions
                                    .insert(device_id, (target_addr_clone, source_clone, bitrate));
                            }
                            Err(e) => {
                                tracing::error!("[Engine] Bitrate change failed: {}", e);
                            }
                        }
                    }
                }
                AudioStreamCommand::GetTcpBroadcaster { device_id, reply } => {
                    tracing::info!("[Engine] GetTcpBroadcaster for device={:?}", device_id);
                    if let Some((_target_addr, source, bitrate)) =
                        self.active_receiver_sessions.get(&device_id)
                    {
                        match self.pool.subscribe(source.clone(), None, *bitrate).await {
                            Ok(broadcast_tx) => {
                                let _ = reply.send(Some(broadcast_tx));
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

    pub fn get_tcp_broadcaster(
        &self,
        source: &AudioSource,
    ) -> Option<broadcast::Sender<Arc<Vec<u8>>>> {
        self.pool.get_tcp_broadcaster(source)
    }
}
