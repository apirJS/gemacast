use std::net::IpAddr;

use tokio::sync::{Mutex, mpsc};

use crate::control::types::{WsCommand, WsEvent};
use crate::control::ws_session::WebSocketSession;
#[cfg(test)]
use crate::control::ws_session::close_websocket;
use crate::domain::error::{ControlError, GemaCastError};

/// Receives control events and sends commands over one pinned WebSocket.
pub struct WsControlClient {
    command_tx: mpsc::Sender<WsCommand>,
    event_rx: Mutex<mpsc::Receiver<Result<WsEvent, GemaCastError>>>,
}

impl WsControlClient {
    pub async fn new(target_ip: IpAddr, device_id: &str) -> Result<Self, GemaCastError> {
        Self::new_with_token(target_ip, device_id, None).await
    }

    pub async fn new_with_token(
        target_ip: IpAddr,
        device_id: &str,
        token: Option<&str>,
    ) -> Result<Self, GemaCastError> {
        Self::new_with_credentials(target_ip, device_id, token, None).await
    }

    pub async fn new_with_credentials(
        target_ip: IpAddr,
        device_id: &str,
        token: Option<&str>,
        pc_certificate_fingerprint: Option<&str>,
    ) -> Result<Self, GemaCastError> {
        let (command_tx, event_rx) =
            WebSocketSession::connect(target_ip, device_id, token, pc_certificate_fingerprint)
                .await?;

        Ok(Self {
            command_tx,
            event_rx: Mutex::new(event_rx),
        })
    }

    pub async fn recv_event(&self) -> Result<WsEvent, GemaCastError> {
        let mut event_guard = self.event_rx.lock().await;

        match event_guard.recv().await {
            Some(Ok(event)) => Ok(event),
            Some(Err(error)) => Err(error),
            None => Err(ControlError::Rejected {
                reason: "Background WebSocket task terminated unexpectedly".into(),
            }
            .into()),
        }
    }

    pub async fn send_disconnect_command(&self) -> Result<(), GemaCastError> {
        self.command_tx
            .send(WsCommand::Disconnect)
            .await
            .map_err(|_| {
                ControlError::WebSocketFailed {
                    reason: "Background WebSocket task is disconnected".into(),
                }
                .into()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;

    #[tokio::test]
    async fn bearer_token_should_require_a_certificate_pin() {
        let result = WsControlClient::new_with_credentials(
            "127.0.0.1".parse().unwrap(),
            "phone-1",
            Some("secret-token"),
            None,
        )
        .await;

        assert!(matches!(
            result,
            Err(error) if error.to_string().contains("without a pinned PC certificate")
        ));
    }

    #[tokio::test]
    async fn cooperative_shutdown_sends_a_websocket_close_frame() {
        let (client_io, server_io) = tokio::io::duplex(1024);
        let mut client = WebSocketStream::from_raw_socket(
            client_io,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let mut server = WebSocketStream::from_raw_socket(
            server_io,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;

        let server_task = tokio::spawn(async move {
            let received_close = matches!(server.next().await, Some(Ok(Message::Close(_))));
            let _ = server.close(None).await;
            received_close
        });

        close_websocket(&mut client).await;

        assert!(server_task.await.unwrap());
    }
}
