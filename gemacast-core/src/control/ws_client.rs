use futures::{SinkExt, StreamExt};
use std::net::IpAddr;
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::control::types::{WsCommand, WsEvent};
use crate::{
    error::{ControlError, GemaCastError},
    network::Ports,
};

pub struct WsControlClient {
    command_tx: mpsc::Sender<WsCommand>,
    event_rx: Mutex<mpsc::Receiver<Result<WsEvent, GemaCastError>>>,
}

impl WsControlClient {
    pub async fn new(target_ip: IpAddr, device_id: &str) -> Result<Self, GemaCastError> {
        let url =
            Url::parse(&format!("ws://{}:{}/ws?device_id={}", target_ip, Ports::CONTROL, device_id)).map_err(|e| {
                ControlError::WebSocketFailed {
                    reason: format!("failed to parse WS URL: {e}"),
                }
            })?;

        let (ws_stream, _) =
            connect_async(url.as_str())
                .await
                .map_err(|e| ControlError::WebSocketFailed {
                    reason: format!("failed to initiate WS connection: {e}"),
                })?;

        let (mut ws_write, mut ws_read) = ws_stream.split();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Result<WsEvent, GemaCastError>>(32);
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel::<WsCommand>(32);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = ws_read.next() => {
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
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        let err = ControlError::Serialization(e).into();
                                        if event_tx.send(Err(err)).await.is_err() {
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
                            _ => continue
                        }
                    },

                    cmd = command_rx.recv() => {
                        let Some(cmd) = cmd else {
                            break;
                        };

                        match serde_json::to_string(&cmd) {
                            Ok(cmd_string) => {
                                if ws_write.send(Message::text(cmd_string)).await.is_err() {
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
