use crate::control::{
    ControlServerState, SessionGeneration,
    types::{WsCommand, WsEvent},
};
use crate::domain::types::DeviceId;
use crate::ports::process_lister::ProcessLister;
use axum::extract::ws::{Message, WebSocket};
use futures::SinkExt;
use futures::stream::StreamExt;
use tokio::sync::mpsc;

pub async fn handle_ws<P: ProcessLister + 'static>(
    socket: WebSocket,
    device_id: DeviceId,
    generation: SessionGeneration,
    state: ControlServerState<P>,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (event_tx, event_rx) = mpsc::channel::<WsEvent>(32);
    let mut event_rx = event_rx;

    {
        let mut connections = state.ws_connections.lock().unwrap();
        connections.insert(device_id.clone(), event_tx.clone());
    }

    loop {
        tokio::select! {
            msg_result = ws_receiver.next() => {
                let Some(msg_result) = msg_result else {
                    break;
                };
                match msg_result {
                    Ok(Message::Text(text)) => {
                        tracing::info!("WS Message received from {}: {}", device_id, text);
                        if let Err(e) = handle_ws_command(&text, &device_id, generation, &state).await {
                            tracing::error!("WebSocket command error for device {}: {}", device_id, e);
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(error) => {
                        if is_expected_disconnect(&error) {
                            tracing::debug!("WebSocket peer disconnected for device {}: {}", device_id, error);
                        } else {
                            tracing::warn!("WebSocket error for device {}: {}", device_id, error);
                        }
                        break;
                    }
                    _ => {}
                }
            }
            event = event_rx.recv() => {
                let Some(event) = event else {
                    break;
                };
                let msg = match serde_json::to_string(&event) {
                    Ok(json) => json,
                    Err(error) => {
                        tracing::error!("Failed to serialize WsEvent: {}", error);
                        continue;
                    }
                };
                match ws_sender.send(Message::Text(msg.clone().into())).await {
                    Ok(()) => tracing::info!("WS Event sent: {}", msg),
                    Err(error) => {
                        if is_expected_disconnect(&error) {
                            tracing::debug!("WebSocket peer disconnected while sending to device {}: {}", device_id, error);
                        } else {
                            tracing::warn!("WebSocket send error for device {}: {}", device_id, error);
                        }
                        break;
                    }
                }
            }
        }
    }

    let is_current = {
        let mut connections = state.ws_connections.lock().unwrap();
        let is_match = connections
            .get(&device_id)
            .is_some_and(|tx| tx.same_channel(&event_tx));

        if is_match {
            connections.remove(&device_id);
        }
        is_match
    };

    // The WebSocket is optional control-plane state, not the stream's liveness
    // authority. HTTPS probes and the audio transport own teardown, so an
    // incidental WS drop must not interrupt a healthy stream. The generation
    // check above still prevents an old socket from removing a newer map entry.
    let _ = (is_current, generation);

    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), ws_sender.close()).await;
}

fn is_expected_disconnect(error: &axum::Error) -> bool {
    let mut current: &(dyn std::error::Error + 'static) = error;
    loop {
        if let Some(error) = current.downcast_ref::<tokio_tungstenite::tungstenite::Error>() {
            use tokio_tungstenite::tungstenite::Error;
            use tokio_tungstenite::tungstenite::error::ProtocolError;

            return match error {
                Error::ConnectionClosed => true,
                Error::Protocol(ProtocolError::ResetWithoutClosingHandshake) => true,
                Error::Io(error) => matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::NotConnected
                ),
                _ => false,
            };
        }
        if let Some(error) = current.downcast_ref::<std::io::Error>() {
            return matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
            );
        }
        let Some(source) = current.source() else {
            return false;
        };
        current = source;
    }
}

async fn handle_ws_command<P: ProcessLister + 'static>(
    text: &str,
    device_id: &DeviceId,
    generation: SessionGeneration,
    state: &ControlServerState<P>,
) -> Result<(), String> {
    let command: WsCommand =
        serde_json::from_str(text).map_err(|e| format!("Failed to parse WsCommand: {}", e))?;

    match command {
        WsCommand::Disconnect => {
            // Check if this command came from the current active WebSocket.
            // We can do this by checking if it's still in the map?
            // Actually, for explicit Disconnect from the client, we should probably
            // just process it. But to be safe against delayed packets, let's process it.
            let dummy_addr = "0.0.0.0:0".parse().unwrap();
            let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
            let _ = state
                .command_tx
                .send(crate::control::ControlCommand::Disconnect {
                    device_id: device_id.clone(),
                    remote_addr: dummy_addr,
                    generation: Some(generation),
                    response_tx,
                })
                .await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abrupt_peer_eof_is_an_expected_control_channel_disconnect() {
        let error = axum::Error::new(tokio_tungstenite::tungstenite::Error::Io(
            std::io::Error::from(std::io::ErrorKind::UnexpectedEof),
        ));

        assert!(is_expected_disconnect(&error));
    }

    #[test]
    fn protocol_corruption_is_not_an_expected_disconnect() {
        let error = axum::Error::new(tokio_tungstenite::tungstenite::Error::Protocol(
            tokio_tungstenite::tungstenite::error::ProtocolError::InvalidOpcode(3),
        ));

        assert!(!is_expected_disconnect(&error));
    }
}
