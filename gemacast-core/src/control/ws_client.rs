use futures::{SinkExt, StreamExt};
use std::net::IpAddr;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Error as WebSocketError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{Connector, connect_async_tls_with_config};
use url::Url;

use crate::control::types::{WsCommand, WsEvent};
use crate::{
    domain::error::{ControlError, GemaCastError},
    network::Ports,
};

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
        if token.is_some() && pc_certificate_fingerprint.is_none() {
            return Err(ControlError::WebSocketFailed {
                reason: "refusing to send a WebSocket token without a pinned PC certificate".into(),
            }
            .into());
        }
        let url = Url::parse(&format!(
            "wss://{}:{}/ws?device_id={}",
            target_ip,
            Ports::CONTROL,
            device_id
        ))
        .map_err(|e| ControlError::WebSocketFailed {
            reason: format!("failed to parse WS URL: {e}"),
        })?;

        let mut request =
            url.as_str()
                .into_client_request()
                .map_err(|e| ControlError::WebSocketFailed {
                    reason: format!("failed to build WS request: {e}"),
                })?;
        if let Some(token) = token {
            let value =
                format!("Bearer {token}")
                    .parse()
                    .map_err(|e| ControlError::WebSocketFailed {
                        reason: format!("failed to build WS authorization header: {e}"),
                    })?;
            request.headers_mut().insert("Authorization", value);
        }

        let tls_config =
            crate::control::tls::client_config(pc_certificate_fingerprint).map_err(|reason| {
                ControlError::WebSocketFailed {
                    reason: format!("failed to configure WSS certificate pin: {reason}"),
                }
            })?;
        let (ws_stream, _) = connect_async_tls_with_config(
            request,
            None,
            false,
            Some(Connector::Rustls(std::sync::Arc::new(tls_config))),
        )
        .await
        .map_err(|e| ControlError::WebSocketFailed {
            reason: format!("failed to initiate WSS connection: {e}"),
        })?;

        let mut ws_stream = ws_stream;
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Result<WsEvent, GemaCastError>>(32);
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel::<WsCommand>(32);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = ws_stream.next() => {
                        let Some(msg) = msg else {
                            let err = Err(ControlError::Rejected {
                                 reason: "WebSocket connection dropped".into()
                            }.into());

                            let _ = event_tx.send(err).await;
                            break;
                        };


                        match msg {
                            Ok(Message::Text(text)) => {
                                match serde_json::from_str::<WsEvent>(&text) {
                                    Ok(event) => {
                                        if event_tx.send(Ok(event)).await.is_err() {
                                            close_websocket(&mut ws_stream).await;
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        let err = ControlError::Serialization(e).into();
                                        if event_tx.send(Err(err)).await.is_err() {
                                            close_websocket(&mut ws_stream).await;
                                            break;
                                        }
                                    }
                                }
                            },
                            Ok(Message::Close(_)) => {
                                let _ = event_tx.send(Err(ControlError::Rejected {
                                    reason: "WS Closed cleanly".into()
                                }.into())).await;

                                break;
                            }
                            Err(error) => {
                                let _ = event_tx.send(Err(ControlError::WebSocketFailed {
                                    reason: format!("WebSocket receive failed: {error}"),
                                }.into())).await;
                                break;
                            }
                            _ => continue,
                        }
                    },

                    cmd = command_rx.recv() => {
                        let Some(cmd) = cmd else {
                            close_websocket(&mut ws_stream).await;
                            break;
                        };

                        match serde_json::to_string(&cmd) {
                            Ok(cmd_string) => {
                                if let Err(error) = ws_stream.send(Message::text(cmd_string)).await {
                                    let _ = event_tx.send(Err(ControlError::WebSocketFailed {
                                        reason: format!("WebSocket send failed: {error}"),
                                    }.into())).await;
                                    break;
                                }
                            }
                            Err(e) => {
                                let err = ControlError::Serialization(e).into();
                                let _ = event_tx.send(Err(err)).await;
                            }
                        }
                    }

                }
            }
        });

        Ok(Self {
            command_tx,
            event_rx: Mutex::new(event_rx),
        })
    }

    pub async fn recv_event(&self) -> Result<WsEvent, GemaCastError> {
        let mut event_guard = self.event_rx.lock().await;

        match event_guard.recv().await {
            Some(Ok(event)) => Ok(event),
            Some(Err(e)) => Err(e),
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

async fn close_websocket<S>(ws_stream: &mut WebSocketStream<S>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    const CLOSE_TIMEOUT: Duration = Duration::from_millis(750);

    let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
        if ws_stream.close(None).await.is_err() {
            return;
        }
        while let Some(message) = ws_stream.next().await {
            match message {
                Ok(Message::Close(_))
                | Err(WebSocketError::ConnectionClosed)
                | Err(WebSocketError::AlreadyClosed) => break,
                Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

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
