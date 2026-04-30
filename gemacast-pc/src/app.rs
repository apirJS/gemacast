use crate::events::{DaemonEvent, StreamCommand};
use crate::tray::TrayManager;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::MenuEvent;

fn display_error_dialog(message: String) {
    rfd::MessageDialog::new()
        .set_title("Gemacast Error!")
        .set_description(message)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

pub fn run() {
    let state = crate::state::create_shared_state();
    let event_loop = EventLoopBuilder::<DaemonEvent>::with_user_event().build();
    let (stream_command_tx, stream_command_rx) = tokio::sync::mpsc::channel::<StreamCommand>(32);
    let state_for_bg = state.clone();

    crate::network::spawn_background_engine(
        event_loop.create_proxy(),
        state_for_bg,
        stream_command_rx,
    );

    let _ = stream_command_tx.try_send(StreamCommand::StartBroadcasting);
    let mut tray_manager = TrayManager::new();
    let proxy_for_main = event_loop.create_proxy();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(daemon_event) => match daemon_event {
                DaemonEvent::DiscoveredDevice {
                    device_id,
                    name,
                    addr,
                } => {
                    let transport = state
                        .lock()
                        .ok()
                        .and_then(|map| map.get(&device_id).and_then(|d| d.transport));
                    tray_manager.add_device(device_id.clone(), &name, addr, transport);
                    tray_manager.set_device_connected(&device_id, true);
                }
                DaemonEvent::DeviceLost(device_id, _addr) => {
                    tray_manager.set_device_connected(&device_id, false);
                    tray_manager.remove_device(&device_id);
                }
                DaemonEvent::FatalError(error_msg) => {
                    display_error_dialog(error_msg);
                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::MainEventsCleared => {
                if let Ok(menu_event) = MenuEvent::receiver().try_recv() {
                    let mut clicked_device_id = None;
                    for (device_id, menu_item) in &tray_manager.device_buttons {
                        if menu_item.id() == menu_event.id() {
                            clicked_device_id = Some(device_id.clone());
                            break;
                        }
                    }

                    let mut clicked_quality = None;
                    for (bitrate_opt, menu_item) in &tray_manager.quality_buttons {
                        if menu_item.id() == menu_event.id() {
                            clicked_quality = Some(*bitrate_opt);
                            break;
                        }
                    }

                    if let Some(new_bitrate) = clicked_quality {
                        for (bitrate_opt, menu_item) in &tray_manager.quality_buttons {
                            menu_item.set_checked(*bitrate_opt == new_bitrate);
                        }
                        if let Err(e) =
                            stream_command_tx.try_send(StreamCommand::ChangeBitrate(new_bitrate))
                        {
                            display_error_dialog(e.to_string());
                        }
                    }

                    if menu_event.id() == tray_manager.broadcast_toggle.id() {
                        let label = tray_manager.broadcast_toggle.text();
                        let currently_broadcasting = label.contains("Stop");

                        let command = if currently_broadcasting {
                            StreamCommand::StopBroadcasting
                        } else {
                            StreamCommand::StartBroadcasting
                        };

                        if let Err(e) = stream_command_tx.try_send(command) {
                            display_error_dialog(format!("Failed to toggle streaming: {}", e));
                        } else {
                            if currently_broadcasting {
                                tray_manager.broadcast_toggle.set_text("Start Stream");
                            } else {
                                tray_manager.broadcast_toggle.set_text("Stop Stream");
                            }
                        }
                    }

                    if let Some(device_id) = clicked_device_id {
                        let addr_opt = state.lock().ok().and_then(|map| {
                            map.get(&device_id).map(|d| (d.device_id.clone(), d.addr))
                        });

                        if let Some((_dev_id, addr)) = addr_opt {
                            if let Err(e) = stream_command_tx
                                .try_send(StreamCommand::RemoveTarget(addr, device_id.clone()))
                            {
                                display_error_dialog(e.to_string());
                            }
                            let _ =
                                proxy_for_main.send_event(DaemonEvent::DeviceLost(device_id, addr));
                        }
                    }

                    if menu_event.id() == tray_manager.quit_item.id() {
                        let _ = stream_command_tx.try_send(StreamCommand::StopStream);
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            Event::LoopDestroyed => {
                let _ = stream_command_tx.try_send(StreamCommand::StopStream);
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            _ => {}
        }
    });
}
