use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::{TlsAcceptor, server::TlsStream};

use crate::control::handlers::{
    handle_change_bitrate, handle_change_source, handle_connect, handle_disconnect,
    handle_get_processes, handle_get_sources, handle_probe, handle_ws_upgrade,
};
use crate::control::state::ControlServerState;
use crate::domain::error::{ControlError, GemaCastError, NetworkError};
use crate::network::Ports;
use crate::ports::process_lister::ProcessLister;

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn build_router<P: ProcessLister + Clone + 'static>(
    state: ControlServerState<P>,
) -> Router {
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
    let listener =
        TcpListener::bind(addr)
            .await
            .map_err(|source| NetworkError::SocketBindFailed {
                addr: addr.to_string(),
                source,
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
pub(crate) struct PeerAddr(pub(crate) SocketAddr);

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
pub(crate) struct PlainListener(pub(crate) TcpListener);

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
