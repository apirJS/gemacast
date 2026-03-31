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
    let (stream_command_tx, stream_command_rx) =
        tokio::sync::mpsc::channel::<StreamCommand>(32);
    let state_for_bg = state.clone();

    crate::network::spawn_background_engine(
        event_loop.create_proxy(),
        state_for_bg,
        stream_command_rx,
    );

    let mut tray_manager = TrayManager::new();
    let state_for_tao = state.clone();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(daemon_event) => match daemon_event {
                DaemonEvent::DiscoveredDevice {
                    device_id,
                    name,
                    addr,
                } => {
                    tray_manager.add_device(device_id, &name, addr);
                }
                DaemonEvent::DeviceLost(device_id, addr) => {
                    let was_active = tray_manager.active_devices.contains(&device_id);
                    tray_manager.remove_device(&device_id);

                    if was_active {
                        if let Err(e) = stream_command_tx.try_send(StreamCommand::RemoveTarget(addr)) {
                            display_error_dialog(e.to_string());
                        }
                    }
                }
                DaemonEvent::FatalError(error_msg) => {
                    display_error_dialog(error_msg);

                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::MainEventsCleared => {
                if let Ok(menu_event) = MenuEvent::receiver().try_recv() {
                    let mut clicked_id = None;
                    for (device_id, menu_item) in &tray_manager.device_buttons {
                        if menu_item.id() == menu_event.id() {
                            clicked_id = Some(device_id.clone());
                            break;
                        }
                    }

                    if let Some(device_id) = clicked_id {
                        let map = state_for_tao.lock().unwrap();
                        if let Some(device) = map.get(&device_id) {
                            let is_now_active = tray_manager.toggle_active_device(&device_id);
                            let command = if is_now_active {
                                StreamCommand::AddTarget(device.addr)
                            } else {
                                StreamCommand::RemoveTarget(device.addr)
                            };
                            
                            if let Err(e) = stream_command_tx.try_send(command) {
                                display_error_dialog(e.to_string());
                            }
                        }
                    }

                    if menu_event.id() == tray_manager.quit_item.id() {
                        let _ = stream_command_tx.try_send(StreamCommand::StopStream);
                        *control_flow = ControlFlow::Exit
                    }
                }
            }
            _ => {}
        }
    });
}
