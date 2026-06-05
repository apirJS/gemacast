//! Tray application event loop.
//!
//! Runs the `tao` event loop on the main thread, processing [`TrayEvent`]s
//! from background tasks and [`MenuEvent`]s from user clicks on the system tray.

use crate::events::{AppCommand, TrayEvent};
use crate::tray::TrayManager;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::MenuEvent;

/// Display a native error dialog to the user.
fn display_error_dialog(message: String) {
    rfd::MessageDialog::new()
        .set_title("Gemacast Error!")
        .set_description(message)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

/// Wait for any termination signal (Ctrl+C, stdin "quit", or OS-specific signals).
async fn wait_for_termination(proxy: EventLoopProxy<TrayEvent>) {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    // Ctrl+C
    let tx = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx.send(()).await;
    });

    #[cfg(windows)]
    {
        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut signal) = tokio::signal::windows::ctrl_close() {
                signal.recv().await;
                let _ = tx.send(()).await;
            }
        });

        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut signal) = tokio::signal::windows::ctrl_break() {
                signal.recv().await;
                let _ = tx.send(()).await;
            }
        });
    }

    #[cfg(unix)]
    {
        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut sigterm) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                sigterm.recv().await;
                let _ = tx.send(()).await;
            }
        });
    }

    // Stdin "quit" command
    let tx = shutdown_tx.clone();
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        while let Ok(bytes) = stdin.read_line(&mut line) {
            if bytes == 0 {
                break;
            }
            if line.trim().eq_ignore_ascii_case("quit") {
                let _ = tx.blocking_send(());
                break;
            }
            line.clear();
        }
    });

    let _ = shutdown_rx.recv().await;
    let _ = proxy.send_event(TrayEvent::ShutdownRequested);
}

/// Run the tray application event loop (blocks the main thread).
pub fn run() {
    let event_loop = EventLoopBuilder::<TrayEvent>::with_user_event().build();

    let (command_tx, command_rx) = tokio::sync::mpsc::channel::<AppCommand>(32);

    let proxy_for_bg = event_loop.create_proxy();
    crate::background::spawn_background_engine(proxy_for_bg, command_rx);

    let proxy_for_term = event_loop.create_proxy();
    std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime for termination listener");

        rt.block_on(async {
            wait_for_termination(proxy_for_term).await;
        });
    });

    let _ = command_tx.try_send(AppCommand::StartBroadcasting);

    let mut tray_manager = match TrayManager::new() {
        Ok(t) => t,
        Err(e) => {
            display_error_dialog(format!("Failed to initialize tray icon: {e}"));
            std::process::exit(1);
        }
    };

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(tray_event) => {
                handle_tray_event(tray_event, &mut tray_manager, &command_tx, control_flow);
            }
            Event::MainEventsCleared => {
                if let Ok(menu_event) = MenuEvent::receiver().try_recv() {
                    handle_menu_event(
                        menu_event.id(),
                        &mut tray_manager,
                        &command_tx,
                        control_flow,
                    );
                }
            }
            Event::LoopDestroyed => {
                let _ = command_tx.try_send(AppCommand::StopAllStreams);
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            _ => {}
        }
    });
}

/// Process a [`TrayEvent`] from the background engine.
fn handle_tray_event(
    event: TrayEvent,
    tray: &mut TrayManager,
    command_tx: &tokio::sync::mpsc::Sender<AppCommand>,
    control_flow: &mut ControlFlow,
) {
    match event {
        TrayEvent::DiscoveredDevice {
            device_id,
            name,
            addr,
            transport,
        } => {
            tray.add_device(device_id.clone(), &name, addr, transport);
            tray.set_device_connected(&device_id, true);
        }
        TrayEvent::DeviceLost { device_id, .. } => {
            tray.set_device_connected(&device_id, false);
            tray.remove_device(&device_id);
        }
        TrayEvent::FatalError(message) => {
            display_error_dialog(message);
        }
        TrayEvent::ShutdownRequested => {
            eprintln!("Shutdown requested. Tearing down gracefully...");
            let _ = command_tx.try_send(AppCommand::ExitApp);
        }
        TrayEvent::ShutdownComplete => {
            eprintln!("Shutdown complete. Exiting.");
            *control_flow = ControlFlow::Exit;
        }
    }
}

/// Process a menu click from the system tray.
fn handle_menu_event(
    menu_event: &tray_icon::menu::MenuId,
    tray: &mut TrayManager,
    command_tx: &tokio::sync::mpsc::Sender<AppCommand>,
    _control_flow: &mut ControlFlow,
) {
    // Check if a device was clicked (to kick it)
    if let Some(device_id) = tray.find_device_by_menu_id(menu_event) {
        if let Err(e) = command_tx.try_send(AppCommand::KickDevice(device_id.clone())) {
            display_error_dialog(e.to_string());
        }
        // Update tray immediately for responsive UI
        tray.set_device_connected(&device_id, false);
        tray.remove_device(&device_id);
        return;
    }

    // Check broadcast toggle
    if *menu_event == tray.broadcast_toggle_item.id() {
        let currently_broadcasting = tray.broadcast_toggle_item.text().contains("Stop");

        let command = if currently_broadcasting {
            AppCommand::StopBroadcasting
        } else {
            AppCommand::StartBroadcasting
        };

        if let Err(e) = command_tx.try_send(command) {
            display_error_dialog(format!("Failed to toggle streaming: {e}"));
        } else if currently_broadcasting {
            tray.broadcast_toggle_item.set_text("Start Stream");
        } else {
            tray.broadcast_toggle_item.set_text("Stop Stream");
        }
        return;
    }

    // Check quit
    if *menu_event == tray.quit_menu_item.id() {
        let _ = command_tx.try_send(AppCommand::ExitApp);
    }
}
