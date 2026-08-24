use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State, ws::WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

use crate::control::auth::{AuthorizedSession, SessionGeneration};
use crate::control::types::{
    ChangeBitrateReq, ChangeSourceReq, ConnectReq, ControlErrorResponse, DisconnectReq,
    PresenceResponse, ProbeReq, ProcessListResponse, SourcesResponse, WsEvent,
};
use crate::domain::error::{ControlError, GemaCastError, NetworkError};
use crate::domain::types::{AudioSource, DeviceId, StreamerCapabilities};
use crate::network::Ports;
use crate::ports::process_lister::ProcessLister;

#[derive(Debug)]
pub enum ControlCommand {
    Connect {
        device_id: DeviceId,
        device_name: String,
        source: Option<AudioSource>,
        remote_addr: SocketAddr,
        bitrate: Option<i32>,
        response_tx: oneshot::Sender<Result<PresenceResponse, String>>,
        authorized: bool,
        pending_request_id: Option<String>,
        device_auth: Option<crate::control::types::DeviceAuthRequest>,
    },
    Disconnect {
        device_id: DeviceId,
        remote_addr: SocketAddr,
        generation: Option<SessionGeneration>,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    GetSources {
        response_tx: oneshot::Sender<SourcesResponse>,
    },
    ChangeSource {
        device_id: DeviceId,
        source: AudioSource,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    ChangeBitrate {
        device_id: DeviceId,
        bitrate: Option<i32>,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    Probe {
        device_id: Option<DeviceId>,
        response_tx: oneshot::Sender<PresenceResponse>,
    },
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(65);
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

fn control_error(
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
) -> axum::response::Response {
    (
        status,
        Json(ControlErrorResponse {
            code: code.to_string(),
            message: message.into(),
        }),
    )
        .into_response()
}

fn connect_error_code(message: &str) -> &'static str {
    let message = message.to_ascii_lowercase();
    if message.contains("cancelled on the phone") {
        "pairing_cancelled"
    } else if message.contains("rejected on the pc") {
        "pairing_rejected"
    } else if message.contains("invalid or expired") || message.contains("challenge expired") {
        "pairing_expired"
    } else if message.contains("too many pending") {
        "pairing_capacity_exhausted"
    } else if message.contains("challenge")
        || message.contains("authentication")
        || message.contains("signature")
        || message.contains("device key")
    {
        "authentication_failed"
    } else if message.contains("remember the approved") {
        "pairing_persistence_failed"
    } else {
        "stream_start_failed"
    }
}

async fn await_mutation(
    response_rx: oneshot::Receiver<Result<(), String>>,
) -> axum::response::Response {
    match tokio::time::timeout(COMMAND_TIMEOUT, response_rx).await {
        Ok(Ok(Ok(()))) => StatusCode::OK.into_response(),
        Ok(Ok(Err(message))) => control_error(StatusCode::CONFLICT, "operation_failed", message),
        Ok(Err(_)) => control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher dropped the acknowledgement",
        ),
        Err(_) => control_error(
            StatusCode::GATEWAY_TIMEOUT,
            "operation_timeout",
            "control operation timed out",
        ),
    }
}

#[derive(Clone)]
pub struct ControlServerState<P: ProcessLister + 'static> {
    pub command_tx: mpsc::Sender<ControlCommand>,
    pub is_broadcasting: Arc<AtomicBool>,
    pub streamer_id: DeviceId,
    pub streamer_name: String,
    pub ws_connections: Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
    pub process_lister: P,
    pub authorizer: crate::control::SessionAuthorizer,
    pub pc_certificate_fingerprint: String,
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn authenticate_device<P: ProcessLister + 'static>(
    state: &ControlServerState<P>,
    headers: &HeaderMap,
    device_id: &DeviceId,
) -> Option<AuthorizedSession> {
    bearer_token(headers).and_then(|token| state.authorizer.authenticate(device_id, token))
}

fn authenticate_token<P: ProcessLister + 'static>(
    state: &ControlServerState<P>,
    headers: &HeaderMap,
) -> Option<AuthorizedSession> {
    bearer_token(headers).and_then(|token| state.authorizer.authenticate_token(token))
}

fn unauthorized() -> axum::response::Response {
    control_error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "a valid device session token is required",
    )
}

impl<P: ProcessLister + 'static> ControlServerState<P> {
    fn build_presence(&self) -> PresenceResponse {
        PresenceResponse {
            device_id: self.streamer_id.clone(),
            streamer_name: self.streamer_name.clone(),
            is_offline: !self.is_broadcasting.load(Ordering::Relaxed),
            pc_network_link: None,
            // This is the fallback used when the dispatcher never answered, so
            // the registry was never consulted. `None` means "unknown", which
            // callers resolve conservatively with a full reconnect.
            device_registered: None,
            session_token: None,
            session_generation: None,
            pending_request_id: None,
            device_auth_challenge: None,
            pc_certificate_fingerprint: Some(self.pc_certificate_fingerprint.clone()),
        }
    }
}

fn build_router<P: ProcessLister + Clone + 'static>(state: ControlServerState<P>) -> Router {
    Router::new()
        .route("/ws", get(handle_ws_upgrade::<P>))
        .route("/probe", post(handle_probe::<P>))
        .route("/connect", post(handle_connect::<P>))
        .route("/disconnect", post(handle_disconnect::<P>))
        .route("/sources", get(handle_get_sources::<P>))
        .route("/processes", get(handle_get_processes::<P>))
        .route("/change-source", post(handle_change_source::<P>))
        .route("/change-bitrate", post(handle_change_bitrate::<P>))
        .with_state(state)
}

pub async fn start_control_server<P: ProcessLister + Clone + 'static>(
    state: ControlServerState<P>,
    tls_config: Arc<rustls::ServerConfig>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), GemaCastError> {
    tracing::info!("Starting HTTPS control server on port {}", Ports::CONTROL);
    let app = build_router(state);
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, Ports::CONTROL);
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

    axum::serve(
        TlsListener::new(listener, tls_config),
        app.into_make_service_with_connect_info::<PeerAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    })
    .await
    .map_err(ControlError::ServerStartFailed)?;

    Ok(())
}

struct TlsListener {
    listener: TcpListener,
    acceptor: TlsAcceptor,
}

#[derive(Debug, Clone, Copy)]
struct PeerAddr(SocketAddr);

impl TlsListener {
    fn new(listener: TcpListener, config: Arc<rustls::ServerConfig>) -> Self {
        Self {
            listener,
            acceptor: TlsAcceptor::from(config),
        }
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<tokio::net::TcpStream>;
    type Addr = PeerAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, addr) = match self.listener.accept().await {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::error!("HTTPS accept failed: {error}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, self.acceptor.accept(stream)).await {
                Ok(Ok(stream)) => return (stream, PeerAddr(addr)),
                Ok(Err(error)) => {
                    tracing::warn!("Rejected invalid TLS connection from {addr}: {error}");
                }
                Err(_) => {
                    tracing::warn!("TLS handshake from {addr} timed out");
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr().map(PeerAddr)
    }
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TlsListener>>
    for PeerAddr
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, TlsListener>) -> Self {
        *stream.remote_addr()
    }
}

#[cfg(test)]
struct PlainListener(TcpListener);

#[cfg(test)]
impl axum::serve::Listener for PlainListener {
    type Io = tokio::net::TcpStream;
    type Addr = PeerAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.0.accept().await {
                Ok((stream, addr)) => return (stream, PeerAddr(addr)),
                Err(error) => {
                    tracing::error!("test control accept failed: {error}");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.0.local_addr().map(PeerAddr)
    }
}

#[cfg(test)]
impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, PlainListener>>
    for PeerAddr
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, PlainListener>) -> Self {
        *stream.remote_addr()
    }
}

async fn handle_probe<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    Json(req): Json<ProbeReq>,
) -> Json<PresenceResponse> {
    tracing::info!("HTTP POST /probe from {:?}", req.device_id);
    let (response_tx, response_rx) = oneshot::channel();
    let _ = state
        .command_tx
        .send(ControlCommand::Probe {
            device_id: req.device_id,
            response_tx,
        })
        .await;

    let mut presence = match response_rx.await {
        Ok(p) => p,
        Err(_) => state.build_presence(),
    };
    presence.pc_certificate_fingerprint = Some(state.pc_certificate_fingerprint.clone());

    Json(presence)
}

async fn handle_connect<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<ConnectReq>,
) -> axum::response::Response {
    tracing::info!("HTTP POST /connect from {:?}", req.device_id);

    let pc_link = Some(crate::network::interface::detect_pc_link(
        req.mode,
        addr.ip(),
    ));

    if !state.is_broadcasting.load(Ordering::Relaxed) {
        return control_error(
            StatusCode::FORBIDDEN,
            "streamer_offline",
            format!("streamer {} is offline", state.streamer_name),
        );
    }

    let current_session = authenticate_device(&state, &headers, &req.device_id);
    let authorized = current_session.is_some();
    let (response_tx, response_rx) = oneshot::channel();
    if state
        .command_tx
        .send(ControlCommand::Connect {
            device_id: req.device_id,
            device_name: req.device_name,
            source: req.source.clone(),
            remote_addr: addr,
            bitrate: req.bitrate,
            response_tx,
            authorized,
            pending_request_id: req.pending_request_id,
            device_auth: req.device_auth,
        })
        .await
        .is_err()
    {
        return control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher is unavailable",
        );
    }

    let mut presence = match tokio::time::timeout(CONNECT_TIMEOUT, response_rx).await {
        Ok(Ok(Ok(presence))) => presence,
        Ok(Ok(Err(message))) => {
            let code = connect_error_code(&message);
            return control_error(StatusCode::CONFLICT, code, message);
        }
        Ok(Err(_)) => {
            return control_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "dispatcher_unavailable",
                "control dispatcher dropped the acknowledgement",
            );
        }
        Err(_) => {
            return control_error(
                StatusCode::GATEWAY_TIMEOUT,
                "operation_timeout",
                "stream start timed out",
            );
        }
    };

    // Inject the PC's detected network link into the response
    presence.pc_network_link = pc_link;
    presence.pc_certificate_fingerprint = Some(state.pc_certificate_fingerprint.clone());

    let status = if presence.session_token.is_none() && presence.pending_request_id.is_some() {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    (status, Json(presence)).into_response()
}

async fn handle_disconnect<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<DisconnectReq>,
) -> axum::response::Response {
    tracing::info!("HTTP POST /disconnect from {:?}", req.device_id);
    let Some(session) = authenticate_device(&state, &headers, &req.device_id) else {
        return unauthorized();
    };
    let (response_tx, response_rx) = oneshot::channel();
    if state
        .command_tx
        .send(ControlCommand::Disconnect {
            device_id: req.device_id,
            remote_addr: addr,
            generation: Some(session.generation),
            response_tx,
        })
        .await
        .is_err()
    {
        return control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher is unavailable",
        );
    }
    await_mutation(response_rx).await
}

async fn handle_get_sources<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
) -> axum::response::Response {
    tracing::info!("HTTP GET /sources");
    let _ = addr;
    if authenticate_token(&state, &headers).is_none() {
        return unauthorized();
    }
    let (response_tx, response_rx) = oneshot::channel();
    let _ = state
        .command_tx
        .send(ControlCommand::GetSources { response_tx })
        .await;

    let response = match response_rx.await {
        Ok(r) => r,
        Err(_) => SourcesResponse {
            sources: vec![AudioSource::Desktop],
            capabilities: StreamerCapabilities {
                supports_process_capture: false,
            },
        },
    };

    Json(response).into_response()
}

async fn handle_change_source<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<ChangeSourceReq>,
) -> axum::response::Response {
    tracing::info!("HTTP POST /change-source from {:?}", req.device_id);
    let _ = addr;
    if authenticate_device(&state, &headers, &req.device_id).is_none() {
        return unauthorized();
    }
    let (response_tx, response_rx) = oneshot::channel();
    if state
        .command_tx
        .send(ControlCommand::ChangeSource {
            device_id: req.device_id,
            source: req.source,
            response_tx,
        })
        .await
        .is_err()
    {
        return control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher is unavailable",
        );
    }
    await_mutation(response_rx).await
}

async fn handle_change_bitrate<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<ChangeBitrateReq>,
) -> axum::response::Response {
    tracing::info!("HTTP POST /change-bitrate from {:?}", req.device_id);
    let _ = addr;
    if authenticate_device(&state, &headers, &req.device_id).is_none() {
        return unauthorized();
    }
    let (response_tx, response_rx) = oneshot::channel();
    if state
        .command_tx
        .send(ControlCommand::ChangeBitrate {
            device_id: req.device_id,
            bitrate: req.bitrate,
            response_tx,
        })
        .await
        .is_err()
    {
        return control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher is unavailable",
        );
    }
    await_mutation(response_rx).await
}

async fn handle_get_processes<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
) -> axum::response::Response {
    tracing::info!("HTTP GET /processes");
    let _ = addr;
    if authenticate_token(&state, &headers).is_none() {
        return unauthorized();
    }
    let processes = state.process_lister.list_processes();
    Json(ProcessListResponse { processes }).into_response()
}

async fn handle_ws_upgrade<P: ProcessLister + 'static>(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    tracing::info!("HTTP GET /ws upgrade request with params: {:?}", params);
    let device_id = match params.get("device_id") {
        Some(id) => DeviceId(id.clone()),
        None => {
            return (StatusCode::BAD_REQUEST, "Missing device_id query parameter").into_response();
        }
    };

    let _ = addr;
    let Some(session) = authenticate_device(&state, &headers, &device_id) else {
        return unauthorized();
    };
    let generation = session.generation;

    ws.on_upgrade(move |socket| crate::control::ws::handle_ws(socket, device_id, generation, state))
}

pub async fn send_ws_event(
    ws_connections: &Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
    device_id: &DeviceId,
    event: WsEvent,
) -> Result<(), GemaCastError> {
    let ws_tx = {
        let connections = ws_connections.lock().unwrap();
        connections.get(device_id).cloned()
    };

    if let Some(tx) = ws_tx {
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

    #[derive(Clone)]
    struct MockProcessLister;
    impl ProcessLister for MockProcessLister {
        fn list_processes(&self) -> Vec<crate::domain::types::ProcessInfo> {
            Vec::new()
        }
    }

    #[test]
    fn connect_errors_should_map_to_stable_client_codes() {
        let cases = [
            ("pairing was cancelled on the phone", "pairing_cancelled"),
            (
                "connection request request-1 was rejected on the PC",
                "pairing_rejected",
            ),
            (
                "connection request request-1 is invalid or expired",
                "pairing_expired",
            ),
            ("device authentication challenge expired", "pairing_expired"),
            (
                "too many pending device-authentication requests",
                "pairing_capacity_exhausted",
            ),
            (
                "device authentication signature is invalid",
                "authentication_failed",
            ),
            (
                "failed to remember the approved device: disk full",
                "pairing_persistence_failed",
            ),
            ("failed to initialize audio capture", "stream_start_failed"),
        ];

        for (message, expected) in cases {
            assert_eq!(connect_error_code(message), expected, "message: {message}");
        }
    }

    async fn spawn_test_server() -> (
        String,
        mpsc::Receiver<ControlCommand>,
        crate::control::SessionAuthorizer,
    ) {
        spawn_test_server_with_broadcasting(true).await
    }

    async fn spawn_test_server_with_broadcasting(
        broadcasting: bool,
    ) -> (
        String,
        mpsc::Receiver<ControlCommand>,
        crate::control::SessionAuthorizer,
    ) {
        let (command_tx, command_rx) = mpsc::channel(10);
        let authorizer = crate::control::SessionAuthorizer::default();
        let state = ControlServerState {
            command_tx,
            is_broadcasting: Arc::new(AtomicBool::new(broadcasting)),
            streamer_id: DeviceId("test-streamer".to_string()),
            streamer_name: "Test Streamer".to_string(),
            ws_connections: Arc::new(Mutex::new(HashMap::new())),
            process_lister: MockProcessLister,
            authorizer: authorizer.clone(),
            pc_certificate_fingerprint: "test-certificate".to_string(),
        };

        let app = build_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            axum::serve(
                PlainListener(listener),
                app.into_make_service_with_connect_info::<PeerAddr>(),
            )
            .await
            .unwrap();
        });

        (format!("http://127.0.0.1:{}", port), command_rx, authorizer)
    }

    #[tokio::test]
    async fn connect_endpoint_should_dispatch_command_and_return_presence() {
        let (base_url, mut command_rx, _) = spawn_test_server().await;
        let client = reqwest::Client::new();

        let req_body = ConnectReq {
            device_id: DeviceId("test-device".to_string()),
            device_name: "Test Device".to_string(),
            source: None,
            bitrate: None,
            jitter_config: crate::domain::types::JitterConfig::default(),
            mode: crate::domain::types::ConnectionMode::Wifi,
            network_link: None,
            pending_request_id: None,
            device_auth: None,
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
                let _ = response_tx.send(Ok(PresenceResponse {
                    device_id,
                    streamer_name: "Test".to_string(),
                    is_offline: false,
                    pc_network_link: None,
                    device_registered: Some(true),
                    session_token: None,
                    session_generation: None,
                    pending_request_id: None,
                    device_auth_challenge: None,
                    pc_certificate_fingerprint: None,
                }));
            }
            _ => panic!("Expected ControlCommand::Connect"),
        }

        let res = request_task.await.unwrap();
        assert!(res.status().is_success());
    }

    #[tokio::test]
    async fn connect_endpoint_should_return_accepted_for_pending_approval() {
        let (base_url, mut command_rx, _) = spawn_test_server().await;
        let client = reqwest::Client::new();
        let req_body = ConnectReq {
            device_id: DeviceId("pending-device".to_string()),
            device_name: "Pending Device".to_string(),
            source: None,
            bitrate: Some(128000),
            jitter_config: crate::domain::types::JitterConfig::default(),
            mode: crate::domain::types::ConnectionMode::Wifi,
            network_link: None,
            pending_request_id: None,
            device_auth: None,
        };

        let request_task = tokio::spawn(async move {
            client
                .post(format!("{}/connect", base_url))
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        match command_rx.recv().await.unwrap() {
            ControlCommand::Connect { response_tx, .. } => {
                let _ = response_tx.send(Ok(PresenceResponse {
                    device_id: DeviceId("test-streamer".into()),
                    streamer_name: "Test Streamer".into(),
                    is_offline: false,
                    pc_network_link: None,
                    device_registered: Some(false),
                    session_token: None,
                    session_generation: None,
                    pending_request_id: Some("request-1".into()),
                    device_auth_challenge: None,
                    pc_certificate_fingerprint: None,
                }));
            }
            _ => panic!("Expected ControlCommand::Connect"),
        }

        let response = request_task.await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        let presence: PresenceResponse = response.json().await.unwrap();
        assert_eq!(presence.pending_request_id.as_deref(), Some("request-1"));
    }

    #[tokio::test]
    async fn change_source_endpoint_should_dispatch_command() {
        let (base_url, mut command_rx, authorizer) = spawn_test_server().await;
        let client = reqwest::Client::new();

        let req_body = ChangeSourceReq {
            device_id: DeviceId("test-device-2".to_string()),
            source: AudioSource::Desktop,
        };

        let (token, _) = authorizer.issue(req_body.device_id.clone()).unwrap();
        let request_task = tokio::spawn(async move {
            client
                .post(format!("{}/change-source", base_url))
                .bearer_auth(token)
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        let cmd = command_rx.recv().await.unwrap();
        match cmd {
            ControlCommand::ChangeSource {
                device_id,
                source,
                response_tx,
            } => {
                assert_eq!(device_id.0, "test-device-2");
                assert_eq!(source, AudioSource::Desktop);
                let _ = response_tx.send(Ok(()));
            }
            _ => panic!("Expected ControlCommand::ChangeSource"),
        }

        let res = request_task.await.unwrap();
        assert!(res.status().is_success());
    }

    #[tokio::test]
    async fn change_bitrate_endpoint_should_dispatch_command() {
        let (base_url, mut command_rx, authorizer) = spawn_test_server().await;
        let client = reqwest::Client::new();

        let req_body = ChangeBitrateReq {
            device_id: DeviceId("test-device-3".to_string()),
            bitrate: Some(192000),
        };

        let (token, _) = authorizer.issue(req_body.device_id.clone()).unwrap();
        let request_task = tokio::spawn(async move {
            client
                .post(format!("{}/change-bitrate", base_url))
                .bearer_auth(token)
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        let cmd = command_rx.recv().await.unwrap();
        match cmd {
            ControlCommand::ChangeBitrate {
                device_id,
                bitrate,
                response_tx,
            } => {
                assert_eq!(device_id.0, "test-device-3");
                assert_eq!(bitrate, Some(192000));
                let _ = response_tx.send(Ok(()));
            }
            _ => panic!("Expected ControlCommand::ChangeBitrate"),
        }

        let res = request_task.await.unwrap();
        assert!(res.status().is_success());
    }

    #[tokio::test]
    async fn connect_endpoint_should_reject_when_not_broadcasting() {
        let (base_url, _command_rx, _) = spawn_test_server_with_broadcasting(false).await;
        let client = reqwest::Client::new();

        let req_body = ConnectReq {
            device_id: DeviceId("test-device".to_string()),
            device_name: "Test Device".to_string(),
            source: None,
            bitrate: None,
            jitter_config: crate::domain::types::JitterConfig::default(),
            mode: crate::domain::types::ConnectionMode::Wifi,
            network_link: None,
            pending_request_id: None,
            device_auth: None,
        };

        let res = client
            .post(format!("{}/connect", base_url))
            .json(&req_body)
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

        let body: ControlErrorResponse = res.json().await.unwrap();
        assert_eq!(body.code, "streamer_offline");
    }

    #[tokio::test]
    async fn probe_endpoint_should_return_presence() {
        let (base_url, mut command_rx, _) = spawn_test_server().await;
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
                    device_id: DeviceId("test-streamer".to_string()),
                    streamer_name: "Test Streamer".to_string(),
                    is_offline: false,
                    pc_network_link: None,
                    device_registered: None,
                    session_token: None,
                    session_generation: None,
                    pending_request_id: None,
                    device_auth_challenge: None,
                    pc_certificate_fingerprint: None,
                });
            }
            _ => panic!("Expected ControlCommand::Probe"),
        }

        let res = request_task.await.unwrap();
        assert!(res.status().is_success());

        let body: PresenceResponse = res.json().await.unwrap();
        assert_eq!(body.device_id.0, "test-streamer");
        assert!(!body.is_offline);
    }
}
