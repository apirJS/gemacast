mod commands;
mod discovery;
mod service;
mod state;

use state::AppState;

/// Seconds after which a sender with no heartbeat is considered offline.
pub(crate) const SENDER_HEARTBEAT_TIMEOUT_SECS: u64 = 10;

/// Interval between watchdog sweeps that check for stale senders.
pub(crate) const HEARTBEAT_CHECK_INTERVAL_SECS: u64 = 1;

/// Initialises and runs the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new())
        .setup(|app| {
            service::spawn_service_command_listener(app.handle().clone());
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_device_info::init())
        .invoke_handler(tauri::generate_handler![
            commands::discovery::get_local_ip,
            commands::discovery::start_listening_for_senders,
            commands::discovery::stop_listening_for_senders,
            commands::playback::connect_to_sender,
            commands::playback::disconnect_from_sender,
            commands::playback::start_audio_playback,
            commands::playback::stop_audio_playback,
            commands::playback::notify_streaming_stopped,
            commands::volume::set_remote_system_volume,
            commands::volume::set_remote_system_mute,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
