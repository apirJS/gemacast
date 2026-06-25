use crate::control::{
    ControlServerState,
    types::{WsCommand, WsEvent},
};
use crate::domain::types::DeviceId;
use crate::ports::process_lister::ProcessLister;
use axum::extract::ws::{Message, WebSocket};
use futures::SinkExt;
use futures::stream::{SplitSink, StreamExt};
use tokio::sync::mpsc;

pub async fn handle_ws<P: ProcessLister + 'static>(
    socket: WebSocket,
    device_id: DeviceId,
    state: ControlServerState<P>,
) {
    let (ws_sender, mut ws_receiver) = socket.split();
    let (event_tx, event_rx) = mpsc::channel::<WsEvent>(32);

    {
        let mut connections = state.ws_connections.lock().unwrap();
        connections.insert(device_id.clone(), event_tx.clone());
    }

    let send_task = tokio::spawn(send_events_to_client(ws_sender, event_rx));

    while let Some(msg_result) = ws_receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                tracing::info!("WS Message received from {}: {}", device_id, text);
                if let Err(e) = handle_ws_command(&text, &device_id, &state).await {
                    tracing::error!("WebSocket command error for device {}: {}", device_id, e);
                }
            }
            Ok(Message::Close(_)) => {
                break;
            }
            Err(e) => {
                tracing::warn!("WebSocket error for device {}: {}", device_id, e);
                break;
            }
            _ => {}
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

    // Always send a disconnect command to ensure Engine cleans up the session
    // even if the WebSocket dropped ungracefully (e.g. unplugged).
    // ONLY send if this was the current active WebSocket for this device.
    if is_current {
        let dummy_addr = "0.0.0.0:0".parse().unwrap();
        let _ = state
            .command_tx
            .send(crate::control::ControlCommand::Disconnect {
                device_id: device_id.clone(),
                remote_addr: dummy_addr,
            })
            .await;
    }

    send_task.abort();
}

async fn send_events_to_client(
    mut ws_sender: SplitSink<WebSocket, Message>,
    mut event_rx: mpsc::Receiver<WsEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        let msg = match serde_json::to_string(&event) {
            Ok(json) => {
                tracing::info!("WS Event sent: {}", json);
                json
            }
            Err(e) => {
                tracing::error!("Failed to serialize WsEvent: {}", e);
                continue;
            }
        };

        if ws_sender.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}

async fn handle_ws_command<P: ProcessLister + 'static>(
    text: &str,
    device_id: &DeviceId,
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
            let _ = state
                .command_tx
                .send(crate::control::ControlCommand::Disconnect {
                    device_id: device_id.clone(),
                    remote_addr: dummy_addr,
                })
                .await;
        }
    }

    Ok(())
}
