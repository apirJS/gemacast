use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Error as WebSocketError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{Connector, WebSocketStream, connect_async_tls_with_config};
use url::Url;

use crate::control::types::{WsCommand, WsEvent};
use crate::domain::error::{ControlError, GemaCastError};
use crate::network::Ports;

/// Establishes a pinned WebSocket connection and owns its background loop.
pub struct WebSocketSession;

impl WebSocketSession {
    pub async fn connect(
        target_ip: IpAddr,
        device_id: &str,
        token: Option<&str>,
        pc_certificate_fingerprint: Option<&str>,
    ) -> Result<
        (
            mpsc::Sender<WsCommand>,
            mpsc::Receiver<Result<WsEvent, GemaCastError>>,
        ),
        GemaCastError,
    > {
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
        .map_err(|error| ControlError::WebSocketFailed {
            reason: format!("failed to parse WS URL: {error}"),
        })?;

        let mut request =
            url.as_str()
                .into_client_request()
                .map_err(|error| ControlError::WebSocketFailed {
                    reason: format!("failed to build WS request: {error}"),
                })?;
        if let Some(token) = token {
            let value = format!("Bearer {token}").parse().map_err(|error| {
                ControlError::WebSocketFailed {
                    reason: format!("failed to build WS authorization header: {error}"),
                }
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
            Some(Connector::Rustls(Arc::new(tls_config))),
        )
        .await
        .map_err(|error| ControlError::WebSocketFailed {
            reason: format!("failed to initiate WSS connection: {error}"),
        })?;

        let (event_tx, event_rx) = mpsc::channel::<Result<WsEvent, GemaCastError>>(32);
        let (command_tx, mut command_rx) = mpsc::channel::<WsCommand>(32);
        let mut ws_stream = ws_stream;

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
                            Ok(Message::Text(text)) => match serde_json::from_str::<WsEvent>(&text) {
                                Ok(event) => {
                                    if event_tx.send(Ok(event)).await.is_err() {
                                        close_websocket(&mut ws_stream).await;
                                        break;
                                    }
                                }
                                Err(error) => {
                                    let err = ControlError::Serialization(error).into();
                                    if event_tx.send(Err(err)).await.is_err() {
                                        close_websocket(&mut ws_stream).await;
                                        break;
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
                    }
                    cmd = command_rx.recv() => {
                        let Some(command) = cmd else {
                            close_websocket(&mut ws_stream).await;
                            break;
                        };

                        match serde_json::to_string(&command) {
                            Ok(command) => {
                                if let Err(error) = ws_stream.send(Message::text(command)).await {
                                    let _ = event_tx.send(Err(ControlError::WebSocketFailed {
                                        reason: format!("WebSocket send failed: {error}"),
                                    }.into())).await;
                                    break;
                                }
                            }
                            Err(error) => {
                                let err = ControlError::Serialization(error).into();
                                let _ = event_tx.send(Err(err)).await;
                            }
                        }
                    }
                }
            }
        });

        Ok((command_tx, event_rx))
    }
}

pub(crate) async fn close_websocket<S>(ws_stream: &mut WebSocketStream<S>)
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
