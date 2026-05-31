use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State, ws::WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use crate::control::types::{
    ChangeBitrateReq, ChangeSourceReq, ConnectReq, DisconnectReq, PresenceResponse, ProbeReq,
    ProcessListResponse, SourcesResponse, WsEvent,
};
use crate::error::{ControlError, GemaCastError, NetworkError};
use crate::network::Ports;
use crate::types::{AudioSource, DeviceId, SenderCapabilities};

#[derive(Debug)]
pub enum ControlCommand {
    Connect {
        device_id: DeviceId,
        device_name: String,
        source: Option<AudioSource>,
        remote_addr: SocketAddr,
        bitrate: Option<i32>,
        response_tx: oneshot::Sender<PresenceResponse>,
    },
    Disconnect {
        device_id: DeviceId,
        remote_addr: SocketAddr,
    },
    GetSources {
        response_tx: oneshot::Sender<SourcesResponse>,
    },
    ChangeSource {
        device_id: DeviceId,
        source: AudioSource,
    },
    ChangeBitrate {
        device_id: DeviceId,
        bitrate: Option<i32>,
    },
    Probe {
        device_id: Option<DeviceId>,
        response_tx: oneshot::Sender<PresenceResponse>,
    },
}

#[derive(Clone)]
pub struct ControlServerState {
    pub command_tx: mpsc::Sender<ControlCommand>,
    pub is_broadcasting: Arc<AtomicBool>,
    pub sender_id: DeviceId,
    pub sender_name: String,
    pub ws_connections: Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
}

impl ControlServerState {
    fn build_presence(&self) -> PresenceResponse {
        PresenceResponse {
            device_id: self.sender_id.clone(),
            sender_name: self.sender_name.clone(),
            is_offline: !self.is_broadcasting.load(Ordering::Relaxed),
        }
    }
}

fn build_router(state: ControlServerState) -> Router {
    Router::new()
        .route("/ws", get(handle_ws_upgrade))
        .route("/probe", post(handle_probe))
        .route("/connect", post(handle_connect))
        .route("/disconnect", post(handle_disconnect))
        .route("/sources", get(handle_get_sources))
        .route("/processes", get(handle_get_processes))
        .route("/change-source", post(handle_change_source))
        .route("/change-bitrate", post(handle_change_bitrate))
        .with_state(state)
}

pub async fn start_control_server(
    state: ControlServerState,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), GemaCastError> {
    let app = build_router(state);
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, Ports::CONTROL);
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    })
    .await
    .map_err(ControlError::ServerStartFailed)?;

    Ok(())
}

async fn handle_probe(
    State(state): State<ControlServerState>,
    Json(req): Json<ProbeReq>,
) -> Json<PresenceResponse> {
    let (response_tx, response_rx) = oneshot::channel();
    let _ = state
        .command_tx
        .send(ControlCommand::Probe {
            device_id: req.device_id,
            response_tx,
        })
        .await;

    let presence = match response_rx.await {
        Ok(p) => p,
        Err(_) => state.build_presence(),
    };

    Json(presence)
}

async fn handle_connect(
    State(state): State<ControlServerState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    Json(req): Json<ConnectReq>,
) -> (StatusCode, Json<PresenceResponse>) {
    if !state.is_broadcasting.load(Ordering::Relaxed) {
        return (
            StatusCode::FORBIDDEN,
            Json(PresenceResponse {
                device_id: state.sender_id.clone(),
                sender_name: state.sender_name.clone(),
                is_offline: true,
            }),
        );
    }

    let (response_tx, response_rx) = oneshot::channel();
    let _ = state
        .command_tx
        .send(ControlCommand::Connect {
            device_id: req.device_id,
            device_name: req.device_name,
            source: req.source.clone(),
            remote_addr: addr,
            bitrate: req.bitrate,
            response_tx,
        })
        .await;

    let presence = match response_rx.await {
        Ok(p) => p,
        Err(_) => state.build_presence(),
    };

    (StatusCode::OK, Json(presence))
}

async fn handle_disconnect(
    State(state): State<ControlServerState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    Json(req): Json<DisconnectReq>,
) -> StatusCode {
    let _ = state
        .command_tx
        .send(ControlCommand::Disconnect {
            device_id: req.device_id,
            remote_addr: addr,
        })
        .await;
    StatusCode::OK
}

async fn handle_get_sources(State(state): State<ControlServerState>) -> Json<SourcesResponse> {
    let (response_tx, response_rx) = oneshot::channel();
    let _ = state
        .command_tx
        .send(ControlCommand::GetSources { response_tx })
        .await;

    let response = match response_rx.await {
        Ok(r) => r,
        Err(_) => SourcesResponse {
            sources: vec![AudioSource::Desktop],
            capabilities: SenderCapabilities {
                supports_process_capture: false,
            },
        },
    };

    Json(response)
}

async fn handle_change_source(
    State(state): State<ControlServerState>,
    Json(req): Json<ChangeSourceReq>,
) -> StatusCode {
    let _ = state
        .command_tx
        .send(ControlCommand::ChangeSource {
            device_id: req.device_id,
            source: req.source,
        })
        .await;
    StatusCode::OK
}

async fn handle_change_bitrate(
    State(state): State<ControlServerState>,
    Json(req): Json<ChangeBitrateReq>,
) -> StatusCode {
    let _ = state
        .command_tx
        .send(ControlCommand::ChangeBitrate {
            device_id: req.device_id,
            bitrate: req.bitrate,
        })
        .await;
    StatusCode::OK
}

async fn handle_get_processes(
    State(_state): State<ControlServerState>,
) -> Json<ProcessListResponse> {
    #[cfg(target_os = "windows")]
    {
        let all_pids =
            unsafe { crate::stream::sender::capture::wasapi_loopback::get_process_list() }
                .unwrap_or_default();

        let audio_pids =
            unsafe { crate::stream::sender::capture::wasapi_loopback::get_audio_process_list() }
                .unwrap_or_default();

        // For each audio-producing PID, walk up the process tree to find the
        // root ancestor with the same executable name. This ensures
        // INCLUDE_TARGET_PROCESS_TREE captures the entire tree's audio —
        // critical for multi-process apps like Chrome where audio is produced
        // by a child renderer process, not the main browser PID.
        let mut audio_root_pids = std::collections::HashSet::<u32>::new();
        for &audio_pid in &audio_pids {
            if let Some(name) = all_pids.get(&audio_pid) {
                let root_pid =
                    crate::stream::sender::capture::wasapi_loopback::get_root_ancestor_pid(
                        audio_pid,
                        &name.to_lowercase(),
                    );
                audio_root_pids.insert(root_pid);
            }
            // Also mark the original audio PID itself
            audio_root_pids.insert(audio_pid);
        }

        // Deduplicate by name: prefer the PID that is a root ancestor of an
        // audio-producing process. Falls back to the lowest PID if no audio
        // session is found for any instance.
        let mut seen = std::collections::HashMap::<String, crate::types::ProcessInfo>::new();
        for (pid, name) in all_pids {
            let key = name.to_lowercase();
            let has_audio = audio_root_pids.contains(&pid);

            seen.entry(key)
                .and_modify(|existing| {
                    // Prefer the PID with an active audio session
                    if has_audio && !existing.has_audio_session {
                        existing.pid = pid;
                        existing.has_audio_session = true;
                    } else if has_audio == existing.has_audio_session && pid < existing.pid {
                        // Same audio status: keep lowest PID for stability
                        existing.pid = pid;
                    }
                })
                .or_insert(crate::types::ProcessInfo {
                    pid,
                    name,
                    has_audio_session: has_audio,
                });
        }

        let mut processes: Vec<_> = seen.into_values().collect();

        // Sort: audio-active processes first, then alphabetically
        processes.sort_by(|a, b| {
            b.has_audio_session
                .cmp(&a.has_audio_session)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        Json(ProcessListResponse { processes })
    }

    #[cfg(not(target_os = "windows"))]
    {
        Json(ProcessListResponse {
            processes: Vec::new(),
        })
    }
}

async fn handle_ws_upgrade(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<ControlServerState>,
) -> impl IntoResponse {
    let device_id = match params.get("device_id") {
        Some(id) => DeviceId(id.clone()),
        None => {
            return (StatusCode::BAD_REQUEST, "Missing device_id query parameter").into_response();
        }
    };

    ws.on_upgrade(|socket| crate::control::ws::handle_ws(socket, device_id, state))
}

pub async fn send_ws_event(
    ws_connections: &Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
    device_id: &DeviceId,
    event: WsEvent,
) -> Result<(), GemaCastError> {
    let sender = {
        let connections = ws_connections.lock().unwrap();
        connections.get(device_id).cloned()
    };

    if let Some(tx) = sender {
        tx.send(event)
            .await
            .map_err(|_| NetworkError::DeviceNotConnected(device_id.0.clone()))?;
        Ok(())
    } else {
        Err(NetworkError::DeviceNotConnected(device_id.0.clone()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn spawn_test_server() -> (String, mpsc::Receiver<ControlCommand>) {
        spawn_test_server_with_broadcasting(true).await
    }

    async fn spawn_test_server_with_broadcasting(
        broadcasting: bool,
    ) -> (String, mpsc::Receiver<ControlCommand>) {
        let (command_tx, command_rx) = mpsc::channel(10);
        let state = ControlServerState {
            command_tx,
            is_broadcasting: Arc::new(AtomicBool::new(broadcasting)),
            sender_id: DeviceId("test-sender".to_string()),
            sender_name: "Test Sender".to_string(),
            ws_connections: Arc::new(Mutex::new(HashMap::new())),
        };

        let app = build_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });

        (format!("http://127.0.0.1:{}", port), command_rx)
    }

    #[tokio::test]
    async fn connect_endpoint_should_dispatch_command_and_return_presence() {
        let (base_url, mut command_rx) = spawn_test_server().await;
        let client = reqwest::Client::new();

        let req_body = ConnectReq {
            device_id: DeviceId("test-device".to_string()),
            device_name: "Test Device".to_string(),
            source: None,
            bitrate: None,
            jitter_config: crate::types::JitterConfig::default(),
            mode: crate::types::ConnectionMode::Wifi,
        };

        let request_task = tokio::spawn(async move {
            client
                .post(format!("{}/connect", base_url))
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        let cmd = command_rx.recv().await.unwrap();
        match cmd {
            ControlCommand::Connect {
                device_id,
                device_name,
                source,
                bitrate,
                response_tx,
                ..
            } => {
                assert_eq!(device_id.0, "test-device");
                assert_eq!(device_name, "Test Device");
                assert!(source.is_none());
                assert!(bitrate.is_none());
                let _ = response_tx.send(PresenceResponse {
                    device_id,
                    sender_name: "Test".to_string(),
                    is_offline: false,
                });
            }
            _ => panic!("Expected ControlCommand::Connect"),
        }

        let res = request_task.await.unwrap();
        assert!(res.status().is_success());
    }

    #[tokio::test]
    async fn change_source_endpoint_should_dispatch_command() {
        let (base_url, mut command_rx) = spawn_test_server().await;
        let client = reqwest::Client::new();

        let req_body = ChangeSourceReq {
            device_id: DeviceId("test-device-2".to_string()),
            source: AudioSource::Desktop,
        };

        let res = client
            .post(format!("{}/change-source", base_url))
            .json(&req_body)
            .send()
            .await
            .unwrap();

        assert!(res.status().is_success());

        let cmd = command_rx.recv().await.unwrap();
        match cmd {
            ControlCommand::ChangeSource { device_id, source } => {
                assert_eq!(device_id.0, "test-device-2");
                assert_eq!(source, AudioSource::Desktop);
            }
            _ => panic!("Expected ControlCommand::ChangeSource"),
        }
    }

    #[tokio::test]
    async fn change_bitrate_endpoint_should_dispatch_command() {
        let (base_url, mut command_rx) = spawn_test_server().await;
        let client = reqwest::Client::new();

        let req_body = ChangeBitrateReq {
            device_id: DeviceId("test-device-3".to_string()),
            bitrate: Some(192000),
        };

        let res = client
            .post(format!("{}/change-bitrate", base_url))
            .json(&req_body)
            .send()
            .await
            .unwrap();

        assert!(res.status().is_success());

        let cmd = command_rx.recv().await.unwrap();
        match cmd {
            ControlCommand::ChangeBitrate { device_id, bitrate } => {
                assert_eq!(device_id.0, "test-device-3");
                assert_eq!(bitrate, Some(192000));
            }
            _ => panic!("Expected ControlCommand::ChangeBitrate"),
        }
    }

    #[tokio::test]
    async fn connect_endpoint_should_reject_when_not_broadcasting() {
        let (base_url, _command_rx) = spawn_test_server_with_broadcasting(false).await;
        let client = reqwest::Client::new();

        let req_body = ConnectReq {
            device_id: DeviceId("test-device".to_string()),
            device_name: "Test Device".to_string(),
            source: None,
            bitrate: None,
            jitter_config: crate::types::JitterConfig::default(),
            mode: crate::types::ConnectionMode::Wifi,
        };

        let res = client
            .post(format!("{}/connect", base_url))
            .json(&req_body)
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

        let body: PresenceResponse = res.json().await.unwrap();
        assert!(body.is_offline);
    }

    #[tokio::test]
    async fn probe_endpoint_should_return_presence() {
        let (base_url, mut command_rx) = spawn_test_server().await;
        let client = reqwest::Client::new();

        let req_body = ProbeReq { device_id: None };

        let request_task = tokio::spawn(async move {
            client
                .post(format!("{}/probe", base_url))
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        let cmd = command_rx.recv().await.unwrap();
        match cmd {
            ControlCommand::Probe {
                device_id,
                response_tx,
            } => {
                assert!(device_id.is_none());
                let _ = response_tx.send(PresenceResponse {
                    device_id: DeviceId("test-sender".to_string()),
                    sender_name: "Test Sender".to_string(),
                    is_offline: false,
                });
            }
            _ => panic!("Expected ControlCommand::Probe"),
        }

        let res = request_task.await.unwrap();
        assert!(res.status().is_success());

        let body: PresenceResponse = res.json().await.unwrap();
        assert_eq!(body.device_id.0, "test-sender");
        assert!(!body.is_offline);
    }
}

