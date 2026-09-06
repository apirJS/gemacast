mod adapters;
mod services;
mod state;
pub mod traits;

#[cfg(test)]
mod testing;

pub(crate) const STREAMER_HEARTBEAT_TIMEOUT_SECS: u64 = 30;

pub(crate) const HEARTBEAT_CHECK_INTERVAL_SECS: u64 = 1;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(tauri_plugin_log::log::LevelFilter::Info)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ))
                .build(),
        )
        .setup(|app| {
            use std::sync::Arc;
            use std::sync::atomic::AtomicBool;
            use tauri::Manager;

            let handle = app.handle().clone();

            let is_streaming = Arc::new(AtomicBool::new(false));

            let notifier: Arc<dyn traits::FrontendNotifier> =
                Arc::new(adapters::TauriFrontendNotifier::new(handle.clone()));

            let platform: Arc<dyn traits::PlatformService> =
                Arc::new(adapters::NativePlatformService::new(handle.clone()));

            let auth_signer: Arc<dyn gemacast_core::control::http_client::DeviceAuthSigner> =
                Arc::new(adapters::PlatformDeviceAuthSigner::new(platform.clone()));

            let client_factory: Arc<dyn traits::StreamerControlClientFactory> =
                Arc::new(adapters::HttpStreamerControlClientFactory::new(auth_signer));

            let session_mgr: Arc<dyn traits::SessionManager> = Arc::new(
                adapters::TokioSessionManager::new(notifier.clone(), client_factory.clone()),
            );

            let network: Arc<dyn traits::NetworkInfoProvider> =
                Arc::new(adapters::NativeNetworkInfoProvider);

            let audio_service = Arc::new(services::audio::service::AudioService {
                session: session_mgr,
                client_factory,
                notifier: notifier.clone(),
                platform: platform.clone(),
                is_streaming: is_streaming.clone(),
                cached_link_pair: std::sync::Mutex::new(None),
                recovery_task: std::sync::Mutex::new(None),
                volume_mix: Arc::new(std::sync::Mutex::new(
                    services::audio::volume::VolumeMix::default(),
                )),
            });

            app.manage(state::AppState {
                audio: audio_service,
                notifier: notifier.clone(),
                network,
                platform,
                discovery_task: tokio::sync::Mutex::new(None),
                is_streaming,
            });

            let cache_dir = handle.path().app_cache_dir().ok();
            tauri::async_runtime::spawn(services::ipc::server::run_service_command_listener(
                notifier, cache_dir,
            ));

            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_device_info::init())
        .invoke_handler(tauri::generate_handler![
            services::updater::commands::check_for_update,
            services::updater::commands::download_update,
            services::updater::commands::install_apk,
            services::updater::commands::cleanup_stale_updates,
            services::discovery::commands::get_local_ip,
            services::discovery::commands::get_network_identifier,
            services::discovery::commands::get_connection_status,
            services::discovery::commands::start_listening_for_streamers,
            services::discovery::commands::stop_listening_for_streamers,
            services::discovery::commands::get_network_state,
            services::discovery::commands::forget_pc_identity,
            services::discovery::commands::get_paired_pc_ids,
            services::audio::commands::connect_to_streamer,
            services::audio::commands::disconnect_from_streamer,
            services::audio::commands::start_audio_playback,
            services::audio::commands::stop_audio_playback,
            services::audio::commands::notify_streaming_stopped,
            services::audio::commands::kill_playback,
            services::audio::commands::update_jitter_config,
            services::audio::commands::get_audio_sources,
            services::audio::commands::change_audio_source,
            services::audio::commands::change_audio_bitrate,
            services::audio::commands::get_process_list,
            services::audio::commands::establish_websocket,
            services::audio::commands::probe_streamer,
            services::audio::commands::start_link_recovery,
            services::audio::commands::stop_link_recovery,
            services::audio::commands::set_audio_gain,
            services::audio::commands::set_match_pc_volume,
            services::audio::commands::get_network_link_pair,
            services::audio::commands::restart_session,
            services::audio::commands::check_exclusive_support,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                static TEARDOWN_STARTED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if TEARDOWN_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    return;
                }

                api.prevent_exit();
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Manager;
                    if let Some(state) = handle.try_state::<state::AppState>() {
                        state.audio.session.stop_session().await;
                    }
                    #[cfg(target_os = "android")]
                    if let Err(error) =
                        services::discovery::native::call_native_finish_and_remove_task(&handle)
                    {
                        tracing::warn!("could not remove the app task on exit: {error}");
                    }
                    handle.exit(0);
                });
            }
        });
}
