use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;

use crate::control::messages::ControlMessage;
use crate::domain::error::GemaCastError;
use crate::domain::types::{DeviceId, TransportType};

pub struct MdnsBroadcaster {
    _daemon: ServiceDaemon,
}

impl MdnsBroadcaster {
    pub fn new(device_id: DeviceId, device_name: String, port: u16) -> Result<Self, GemaCastError> {
        let daemon = ServiceDaemon::new().map_err(|e| {
            GemaCastError::Network(crate::domain::error::NetworkError::MdnsRegisterFailed(e))
        })?;

        let service_type = "_gemacast._tcp.local.";
        // The instance name is the device_id.
        let instance_name = device_id.0.clone();

        // Must be unique per device. We'll use device_id.local.
        let hostname = format!("{}.local.", instance_name);

        let mut properties = HashMap::new();
        properties.insert("device_id".to_string(), device_id.0.clone());
        properties.insert("device_name".to_string(), device_name);

        // We use 0.0.0.0 so that mdns-sd binds to all local interfaces automatically.
        let my_ip = "0.0.0.0";

        let service_info = ServiceInfo::new(
            service_type,
            &instance_name,
            &hostname,
            my_ip,
            port,
            Some(properties),
        )
        .map_err(|_| {
            // ServiceInfo::new only fails if the service type or instance name is invalid.
            GemaCastError::Network(crate::domain::error::NetworkError::MdnsRegisterFailed(
                mdns_sd::Error::Again,
            )) // fallback map
        })?;

        daemon.register(service_info).map_err(|e| {
            GemaCastError::Network(crate::domain::error::NetworkError::MdnsRegisterFailed(e))
        })?;

        Ok(Self { _daemon: daemon })
    }
}

pub struct MdnsListener;

impl MdnsListener {
    pub async fn run(
        incoming_message_tx: tokio::sync::mpsc::Sender<(ControlMessage, std::net::SocketAddr)>,
    ) -> Result<(), GemaCastError> {
        let daemon = ServiceDaemon::new().map_err(|e| {
            GemaCastError::Network(crate::domain::error::NetworkError::MdnsRegisterFailed(e))
        })?;
        let receiver = daemon.browse("_gemacast._tcp.local.").map_err(|e| {
            GemaCastError::Network(crate::domain::error::NetworkError::MdnsRegisterFailed(e))
        })?;

        while let Ok(event) = receiver.recv_async().await {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let props = info.get_properties();
                    let device_id_str = props
                        .get_property_val_str("device_id")
                        .unwrap_or_default()
                        .to_string();
                    let device_name = props
                        .get_property_val_str("device_name")
                        .unwrap_or_default()
                        .to_string();

                    if device_id_str.is_empty() || device_name.is_empty() {
                        continue;
                    }

                    let addrs = info.get_addresses();
                    if let Some(ip) = addrs.iter().next()
                        && let Ok(addr_ip) = ip.to_string().parse::<std::net::IpAddr>()
                    {
                        let addr = std::net::SocketAddr::new(addr_ip, info.get_port());
                        let msg = ControlMessage::Presence {
                            device_id: DeviceId(device_id_str),
                            sender_name: device_name,
                            is_offline: false,
                            transport: Some(TransportType::Wifi),
                        };

                        // Propagate it into the existing listener pipeline
                        let _ = incoming_message_tx.send((msg, addr)).await;
                    }
                }
                ServiceEvent::ServiceRemoved(service_type, fullname) => {
                    // Extract instance name (e.g. `device_id._gemacast._tcp.local.`)
                    let device_id_str = fullname
                        .strip_suffix(&format!(".{}", service_type))
                        .unwrap_or(&fullname)
                        .to_string();

                    // We don't have the IP address anymore, so we construct a dummy one for the channel
                    // The frontend only cares about device_id for offline updates anyway
                    let dummy_addr = std::net::SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)),
                        0,
                    );

                    let msg = ControlMessage::Presence {
                        device_id: DeviceId(device_id_str),
                        sender_name: "Offline".to_string(), // Frontend doesn't care
                        is_offline: true,
                        transport: Some(TransportType::Wifi),
                    };

                    let _ = incoming_message_tx.send((msg, dummy_addr)).await;
                }
                _ => {}
            }
        }

        Ok(())
    }
}
