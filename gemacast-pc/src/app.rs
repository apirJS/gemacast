//! Tray application event loop.
//!
//! Runs the `tao` event loop on the main thread, processing [`TrayEvent`]s
//! from background tasks and [`MenuEvent`]s from user clicks on the system tray.

use crate::events::{AppCommand, TrayEvent};
use crate::tray::TrayManager;
use std::path::PathBuf;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::MenuEvent;

/// Display a native error dialog to the user.
fn display_error_dialog(message: String) {
    tracing::error!("FATAL ERROR: {}", message);
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
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    "Fatal error: Failed to build Tokio runtime for termination listener: {}",
                    e
                );
                std::process::exit(1);
            }
        };

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

    // Path to a downloaded update installer (set by UpdateReady event).
    let mut pending_installer: Option<PathBuf> = None;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(tray_event) => {
                handle_tray_event(
                    tray_event,
                    &mut tray_manager,
                    &command_tx,
                    control_flow,
                    &mut pending_installer,
                );
            }
            Event::MainEventsCleared => {
                if let Ok(menu_event) = MenuEvent::receiver().try_recv() {
                    handle_menu_event(
                        menu_event.id(),
                        &mut tray_manager,
                        &command_tx,
                        control_flow,
                        &mut pending_installer,
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
    pending_installer: &mut Option<PathBuf>,
) {
    match event {
        TrayEvent::UpdateReady {
            version,
            installer_path,
        } => {
            tracing::info!(
                "Update v{} downloaded to {}",
                version,
                installer_path.display()
            );
            tray.show_update_ready(&version);
            *pending_installer = Some(installer_path);
        }
        TrayEvent::UpdateFailed(msg) => {
            tracing::warn!("Update failed: {}", msg);
            tray.show_update_failed();
        }
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
            tracing::info!("Shutdown requested. Tearing down gracefully...");
            let _ = command_tx.try_send(AppCommand::ExitApp);
        }
        TrayEvent::ShutdownComplete => {
            tracing::info!("Shutdown complete. Exiting.");
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
    pending_installer: &mut Option<PathBuf>,
) {
    // --- Update install click ---
    if let Some(ref update_item) = tray.update_menu_item
        && *menu_event == update_item.id()
    {
        if let Some(installer_path) = pending_installer.as_ref() {
            let confirmed = rfd::MessageDialog::new()
                .set_title("Gemacast Update")
                .set_description(
                    "A new version of Gemacast has been downloaded.\n\n\
                     Click OK to install the update now.",
                )
                .set_level(rfd::MessageLevel::Info)
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show();

            if confirmed == rfd::MessageDialogResult::Ok {
                match crate::updater::install_update(installer_path) {
                    Ok(must_exit_now) => {
                        if must_exit_now {
                            // Linux: new binary already spawned, exit immediately.
                            std::process::exit(0);
                        }
                        // Windows/macOS: MSI/DMG launched asynchronously.
                        // Small delay so the installer has time to start before
                        // we exit, avoiding a race where the installer hasn't
                        // launched yet.
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        std::process::exit(0);
                    }
                    Err(e) => {
                        rfd::MessageDialog::new()
                            .set_title("Update Failed")
                            .set_description(format!("Failed to launch installer: {e}"))
                            .set_level(rfd::MessageLevel::Error)
                            .show();
                    }
                }
            } else {
                // User cancelled — clean up the downloaded file.
                crate::updater::cleanup_update(installer_path);
                *pending_installer = None;
                tray.remove_update_item();
            }
        }
        return;
    }

    // --- "Update failed — click to retry" click ---
    if let Some(ref failed_item) = tray.update_failed_menu_item
        && *menu_event == failed_item.id()
    {
        tray.remove_update_failed_item();
        let _ = command_tx.try_send(AppCommand::CheckForUpdates);
        return;
    }

    // --- "Check for Updates" click ---
    if *menu_event == tray.check_update_menu_item.id() {
        let _ = command_tx.try_send(AppCommand::CheckForUpdates);
        return;
    }

    // --- Device click (to kick it) ---
    if let Some(device_id) = tray.find_device_by_menu_id(menu_event) {
        if let Err(e) = command_tx.try_send(AppCommand::KickDevice(device_id.clone())) {
            display_error_dialog(e.to_string());
        }
        // Update tray immediately for responsive UI
        tray.set_device_connected(&device_id, false);
        tray.remove_device(&device_id);
        return;
    }

    // --- Broadcast toggle ---
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

    // --- Launch on Startup toggle ---
    if *menu_event == tray.launch_on_startup_item.id() {
        let new_state = !tray.launch_on_startup_item.is_checked();
        tray.launch_on_startup_item.set_checked(new_state);

        if let Err(e) = crate::autostart::set_autostart(new_state) {
            tracing::warn!("Failed to update autostart: {}", e);
        }

        let mut cfg = crate::config::load_config();
        cfg.launch_on_startup = new_state;
        if let Err(e) = crate::config::save_config(&cfg) {
            tracing::warn!("Failed to save config: {}", e);
        }
        return;
    }

    // --- Quit ---
    if *menu_event == tray.quit_menu_item.id() {
        let _ = command_tx.try_send(AppCommand::ExitApp);
    }
}
