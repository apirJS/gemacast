mod adapters;
mod domains;
mod state;
pub mod traits;

#[cfg(test)]
mod testing;

/// Seconds after which a sender with no heartbeat is considered offline.
pub(crate) const SENDER_HEARTBEAT_TIMEOUT_SECS: u64 = 30;

/// Interval between watchdog sweeps that check for stale senders.
pub(crate) const HEARTBEAT_CHECK_INTERVAL_SECS: u64 = 1;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            use std::sync::Arc;
            use tauri::Manager;

            let handle = app.handle().clone();

            // -- Create production adapters ------------------------------
            let notifier: Arc<dyn traits::FrontendNotifier> =
                Arc::new(adapters::TauriFrontendNotifier::new(handle.clone()));

            let session_mgr: Arc<dyn traits::SessionManager> =
                Arc::new(adapters::TokioSessionManager::new(notifier.clone()));

            let client_factory: Arc<dyn traits::SenderControlClientFactory> =
                Arc::new(adapters::HttpSenderControlClientFactory);

            let platform: Arc<dyn traits::PlatformService> =
                Arc::new(adapters::NativePlatformService::new(handle.clone()));

            let network: Arc<dyn traits::NetworkInfoProvider> =
                Arc::new(adapters::NativeNetworkInfoProvider);

            // -- Wire the AudioService -----------------------------------
            let audio_service = Arc::new(domains::audio::service::AudioService {
                session: session_mgr,
                client_factory,
                notifier: notifier.clone(),
                platform: platform.clone(),
            });

            // -- Register managed state ----------------------------------
            app.manage(state::AppState {
                audio: audio_service,
                notifier: notifier.clone(),
                network,
                platform,
                discovery_task: tokio::sync::Mutex::new(None),
            });

            // -- Spawn IPC listener ----------------------------------
            let cache_dir = handle.path().app_cache_dir().ok();
            tauri::async_runtime::spawn(
                domains::ipc::server::run_service_command_listener(notifier, cache_dir),
            );

            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_device_info::init())
        .invoke_handler(tauri::generate_handler![
            domains::discovery::commands::get_local_ip,
            domains::discovery::commands::get_network_identifier,
            domains::discovery::commands::get_connection_status,
            domains::discovery::commands::start_listening_for_senders,
            domains::discovery::commands::stop_listening_for_senders,
            domains::discovery::commands::get_network_state,
            domains::audio::commands::connect_to_sender,
            domains::audio::commands::disconnect_from_sender,
            domains::audio::commands::start_audio_playback,
            domains::audio::commands::stop_audio_playback,
            domains::audio::commands::notify_streaming_stopped,
            domains::audio::commands::kill_playback,
            domains::audio::commands::update_jitter_config,
            domains::audio::commands::get_audio_sources,
            domains::audio::commands::change_audio_source,
            domains::audio::commands::change_audio_bitrate,
            domains::audio::commands::get_process_list,
            domains::audio::commands::establish_websocket,
            domains::audio::commands::probe_sender,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Manager;
                    if let Some(state) = handle.try_state::<state::AppState>() {
                        state.audio.session.stop_session().await;
                    }
                    handle.exit(0);
                });
            }
        });
}
