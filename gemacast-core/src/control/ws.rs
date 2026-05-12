use crate::control::{
    ControlServerState,
    types::{WsCommand, WsEvent},
};
use crate::types::DeviceId;
use axum::extract::ws::{Message, WebSocket};
use futures::SinkExt;
use futures::stream::{SplitSink, StreamExt};
use tokio::sync::mpsc;

pub async fn handle_ws(socket: WebSocket, device_id: DeviceId, state: ControlServerState) {
    let (ws_sender, mut ws_receiver) = socket.split();
    let (event_tx, event_rx) = mpsc::channel::<WsEvent>(32);

    {
        let mut connections = state.ws_connections.lock().unwrap();
        connections.insert(device_id.clone(), event_tx);
    }

    let send_task = tokio::spawn(send_events_to_client(ws_sender, event_rx));

    while let Some(msg_result) = ws_receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                if let Err(e) = handle_ws_command(&text, &device_id, &state).await {
                    eprintln!("WebSocket command error for device {}: {}", device_id, e);
                }
            }
            Ok(Message::Close(_)) => {
                break;
            }
            Err(e) => {
                eprintln!("WebSocket error for device {}: {}", device_id, e);
                break;
            }
            _ => {}
        }
    }

    {
        let mut connections = state.ws_connections.lock().unwrap();
        connections.remove(&device_id);
    }

    send_task.abort();
}

async fn send_events_to_client(
    mut ws_sender: SplitSink<WebSocket, Message>,
    mut event_rx: mpsc::Receiver<WsEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        let msg = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(e) => {
                eprintln!("Failed to serialize WsEvent: {}", e);
                continue;
            }
        };

        if ws_sender.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}

async fn handle_ws_command(
    text: &str,
    device_id: &DeviceId,
    state: &ControlServerState,
) -> Result<(), String> {
    let command: WsCommand =
        serde_json::from_str(text).map_err(|e| format!("Failed to parse WsCommand: {}", e))?;

    match command {
        WsCommand::Disconnect => {
            // Note: We use a dummy SocketAddr since WebSocket doesn't need it
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
