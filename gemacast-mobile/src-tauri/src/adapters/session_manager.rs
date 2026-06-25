use crate::traits::{FrontendNotifier, SessionInfo, SessionManager, SessionParams};
use async_trait::async_trait;
use gemacast_core::domain::types::{ConnectionMode, JitterConfig};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Internal session state, analogous to the old `state::ActiveSession`.
struct ActiveSession {
    exclusive_mode: bool,
    mode: ConnectionMode,
    bitrate: Option<i32>,
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    jitter_config: Arc<RwLock<JitterConfig>>,
    shutdown_tx: oneshot::Sender<()>,
    playback_task: JoinHandle<()>,
}

/// Manages playback sessions and WebSocket client tasks using Tokio primitives.
pub struct TokioSessionManager {
    notifier: Arc<dyn FrontendNotifier>,
    session: tokio::sync::Mutex<Option<ActiveSession>>,
    ws_client_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl TokioSessionManager {
    pub fn new(notifier: Arc<dyn FrontendNotifier>) -> Self {
        Self {
            notifier,
            session: tokio::sync::Mutex::new(None),
            ws_client_task: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl SessionManager for TokioSessionManager {
    async fn start_session(&self, params: SessionParams) -> Result<(), String> {
        // Tear down any existing session first
        self.stop_session().await;

        let (is_playing, _is_tcp_mode, config_ref, volume, shutdown_tx, playback_task) =
            crate::domains::audio::playback::spawn_session_receiver(
                params.jitter_config.clone(),
                params.is_tcp,
                params.exclusive_mode,
                self.notifier.clone(),
                params.target_ip,
                params.mode,
                params.device_id,
            )?;

        *self.session.lock().await = Some(ActiveSession {
            exclusive_mode: params.exclusive_mode,
            mode: params.mode,
            bitrate: params.bitrate,
            is_playing,
            volume,
            jitter_config: config_ref,
            shutdown_tx,
            playback_task,
        });

        Ok(())
    }

    async fn stop_session(&self) {
        if let Some(session) = self.session.lock().await.take() {
            let _ = session.shutdown_tx.send(());
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(1500),
                session.playback_task,
            )
            .await;
        }
        self.stop_ws_client().await;
    }

    async fn set_playing(&self, playing: bool) {
        if let Some(session) = self.session.lock().await.as_ref() {
            session.is_playing.store(playing, Ordering::Relaxed);
        }
    }

    async fn pause_playback(&self) -> Result<(), String> {
        let guard = self.session.lock().await;
        let session = guard.as_ref().ok_or("No active session")?;
        session.is_playing.store(false, Ordering::Relaxed);
        Ok(())
    }

    async fn resume_playback(&self) -> Result<(), String> {
        let guard = self.session.lock().await;
        let session = guard.as_ref().ok_or("No active session")?;
        session.is_playing.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn update_jitter_config(&self, config: JitterConfig) {
        if let Some(session) = self.session.lock().await.as_ref()
            && let Ok(mut guard) = session.jitter_config.write()
        {
            *guard = config;
        }
    }

    async fn session_info(&self) -> Option<SessionInfo> {
        let guard = self.session.lock().await;
        guard.as_ref().map(|s| SessionInfo {
            exclusive_mode: s.exclusive_mode,
            mode: s.mode,
            bitrate: s.bitrate,
            jitter_config: s
                .jitter_config
                .read()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default(),
        })
    }

    async fn update_bitrate(&self, bitrate: Option<i32>) {
        if let Some(session) = self.session.lock().await.as_mut() {
            session.bitrate = bitrate;
        }
    }

    async fn set_volume(&self, linear: f32) {
        if let Some(session) = self.session.lock().await.as_ref() {
            session
                .volume
                .store(f32::to_bits(linear), Ordering::Relaxed);
        }
    }

    async fn start_ws_client(&self, task: JoinHandle<()>) {
        let mut guard = self.ws_client_task.lock().await;
        if let Some(old_task) = guard.take() {
            old_task.abort();
        }
        *guard = Some(task);
    }

    async fn stop_ws_client(&self) {
        if let Some(task) = self.ws_client_task.lock().await.take() {
            task.abort();
        }
    }
}
