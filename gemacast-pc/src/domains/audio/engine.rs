use crate::events::DaemonEvent;
use gemacast_core::stream::sender::{AudioStreamCommand, AudioStreamEngine};
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

pub fn spawn_audio_engine(
    set: &mut JoinSet<()>,
    engine: AudioStreamEngine,
    audio_engine_command_rx: tokio::sync::mpsc::Receiver<AudioStreamCommand>,
    proxy: EventLoopProxy<DaemonEvent>,
) {
    set.spawn(async move {
        let mut engine = engine;
        if let Err(e) = engine.run_command_loop(audio_engine_command_rx).await {
            let _ = proxy.send_event(DaemonEvent::FatalError(format!(
                "Audio engine failed: {}",
                e
            )));
        }
    });
}
