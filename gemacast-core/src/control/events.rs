use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::control::types::WsEvent;
use crate::domain::error::{GemaCastError, NetworkError};
use crate::domain::types::DeviceId;

/// Delivers control events to the active WebSocket connection registry.
pub struct ControlEventBus;

impl ControlEventBus {
    pub async fn send(
        connections: &Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
        device_id: &DeviceId,
        event: WsEvent,
    ) -> Result<(), GemaCastError> {
        let connection = {
            let connections = connections
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            connections.get(device_id).cloned()
        };

        let Some(connection) = connection else {
            return Err(NetworkError::DeviceNotConnected(device_id.0.clone()).into());
        };

        connection
            .send(event)
            .await
            .map_err(|_| NetworkError::DeviceNotConnected(device_id.0.clone()).into())
    }

    pub async fn broadcast(
        connections: &Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
        event: WsEvent,
    ) -> usize {
        let targets: Vec<mpsc::Sender<WsEvent>> = {
            let connections = connections
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            connections.values().cloned().collect()
        };

        let mut delivered = 0;
        for connection in targets {
            if connection.send(event.clone()).await.is_ok() {
                delivered += 1;
            }
        }
        delivered
    }
}

pub async fn send(
    connections: &Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
    device_id: &DeviceId,
    event: WsEvent,
) -> Result<(), GemaCastError> {
    ControlEventBus::send(connections, device_id, event).await
}

pub async fn broadcast(
    connections: &Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
    event: WsEvent,
) -> usize {
    ControlEventBus::broadcast(connections, event).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[tokio::test]
    async fn send_delivers_an_event_to_the_registered_device() {
        let connections = registry();
        let (sender, mut receiver) = mpsc::channel(1);
        let device_id = DeviceId("phone-1".to_string());
        connections
            .lock()
            .unwrap()
            .insert(device_id.clone(), sender);

        ControlEventBus::send(&connections, &device_id, WsEvent::Disconnect)
            .await
            .unwrap();

        assert!(matches!(receiver.recv().await, Some(WsEvent::Disconnect)));
    }

    #[tokio::test]
    async fn send_returns_a_typed_error_for_an_unregistered_device() {
        let error = ControlEventBus::send(
            &registry(),
            &DeviceId("missing-phone".to_string()),
            WsEvent::Disconnect,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            GemaCastError::Network(NetworkError::DeviceNotConnected(_))
        ));
    }

    #[tokio::test]
    async fn broadcast_reports_only_connections_that_accept_the_event() {
        let connections = registry();
        let (active_sender, mut active_receiver) = mpsc::channel(1);
        let (closed_sender, closed_receiver) = mpsc::channel(1);
        drop(closed_receiver);

        connections
            .lock()
            .unwrap()
            .insert(DeviceId("active".to_string()), active_sender);
        connections
            .lock()
            .unwrap()
            .insert(DeviceId("closed".to_string()), closed_sender);

        assert_eq!(
            ControlEventBus::broadcast(
                &connections,
                WsEvent::Error {
                    message: "test".to_string()
                }
            )
            .await,
            1
        );
        assert!(matches!(
            active_receiver.recv().await,
            Some(WsEvent::Error { message }) if message == "test"
        ));
    }
}
