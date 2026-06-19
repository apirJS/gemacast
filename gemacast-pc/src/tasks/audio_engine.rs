//! Spawns the audio stream engine as a background task.
//!
//! This is a thin wrapper that runs [`AudioStreamEngine::run_command_loop`]
//! and forwards fatal errors to the tray via [`TrayNotifier`].

use std::sync::Arc;

use gemacast_core::stream::sender::AudioStreamCommand;
use gemacast_core::stream::sender::engine::AudioStreamEngine;
use tokio::task::JoinSet;

use crate::traits::TrayNotifier;

/// Spawn the audio stream engine, forwarding fatal errors to the tray.
pub fn spawn_audio_engine(
    set: &mut JoinSet<()>,
    engine: AudioStreamEngine,
    command_rx: tokio::sync::mpsc::Receiver<AudioStreamCommand>,
    tray: Arc<dyn TrayNotifier>,
) {
    set.spawn(async move {
        let mut engine = engine;
        if let Err(e) = engine.run_command_loop(command_rx).await {
            let msg = format!("Audio engine failed: {e}");
            tracing::error!("Fatal error: {}", msg);
            tray.notify_fatal_error(msg);
        }
    });
}
