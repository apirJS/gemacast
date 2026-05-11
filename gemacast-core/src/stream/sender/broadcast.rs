use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use crate::error::GemaCastError;
use crate::types::{AudioSource, DeviceId};

use super::capture_pool::CapturePool;

#[derive(Debug)]
pub enum SenderCommand {
    AddTarget(SocketAddr),
    RemoveTarget(SocketAddr),
    ChangeBitrate(Option<i32>),
}

pub struct StreamEngine {
    pub pool: CapturePool,
    pub receiver_sessions: HashMap<DeviceId, (Option<SocketAddr>, AudioSource)>,
}

pub enum StreamCommand {
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

impl StreamEngine {
    pub fn new(supports_process_capture: bool) -> Self {
        Self {
            pool: CapturePool::new(supports_process_capture),
            receiver_sessions: HashMap::new(),
        }
    }

    pub async fn run_command_loop(
        &mut self,
        mut command_rx: mpsc::Receiver<StreamCommand>,
    ) -> Result<(), GemaCastError> {
        while let Some(command) = command_rx.recv().await {
            match command {
                StreamCommand::Subscribe {
                    device_id,
                    target_addr,
                    source,
                } => {
                    if let Err(_e) = self.pool.subscribe(source.clone(), target_addr).await {
                        continue;
                    }
                    self.receiver_sessions.insert(device_id, (target_addr, source));
                }
                StreamCommand::Unsubscribe { device_id } => {
                    if let Some((target_addr, source)) = self.receiver_sessions.remove(&device_id) {
                        let _ = self.pool.unsubscribe(&source, target_addr).await;
                    }
                }
                StreamCommand::ChangeSource { device_id, source } => {
                    if let Some((target_addr, old_source)) = self.receiver_sessions.get(&device_id) {
                        let old_source = old_source.clone();
                        let target_addr = *target_addr;
                        
                        if let Err(_e) = self.pool.change_source(&old_source, source.clone(), target_addr).await {
                            continue;
                        }
                        
                        self.receiver_sessions.insert(device_id, (target_addr, source));
                    }
                }
                StreamCommand::ChangeBitrate(bitrate_opt) => {
                    self.pool.change_bitrate(bitrate_opt).await;
                }
                StreamCommand::Shutdown => {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn get_tcp_broadcaster(&self, source: &AudioSource) -> Option<broadcast::Sender<Arc<Vec<u8>>>> {
        self.pool.get_tcp_broadcaster(source)
    }
}
