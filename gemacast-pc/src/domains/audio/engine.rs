use gemacast_core::stream::sender::broadcast::{StreamCommand, StreamEngine};
use tokio::task::JoinSet;

pub fn spawn_audio_engine(
    set: &mut JoinSet<()>,
    engine: StreamEngine,
    command_rx: tokio::sync::mpsc::Receiver<StreamCommand>,
) {
    set.spawn(async move {
        let mut engine = engine;
        if let Err(e) = engine.run_command_loop(command_rx).await {
            eprintln!("Audio engine failed: {}", e);
            std::process::exit(1);
        }
    });
}
