use gemacast_core::sender::{AudioSender, SenderCommand};
use tokio::task::JoinSet;

pub fn spawn_audio_engine(
    set: &mut JoinSet<()>,
    engine: AudioSender,
    sender_command_rx: tokio::sync::mpsc::Receiver<SenderCommand>,
    stop_rx: tokio::sync::oneshot::Receiver<()>,
) {
    set.spawn(async move {
        let mut engine = engine;
        if let Err(e) = engine.start_broadcast(sender_command_rx, stop_rx).await {
            eprintln!("Audio engine broadcast failed: {}", e);
            std::process::exit(1);
        }
    });
}
