use super::playback_control::PlaybackCommand;
use super::stream::{PlaybackOutput, PlaybackStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

pub(crate) struct PlaybackWorker;

impl PlaybackWorker {
    pub(crate) fn spawn(
        mut stream: PlaybackStream,
        mut command_rx: mpsc::UnboundedReceiver<PlaybackCommand>,
        render_enabled: Arc<AtomicBool>,
        reset_requested: Arc<AtomicBool>,
        error_tx: mpsc::Sender<String>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut user_wants_playing = true;
            let mut source_idle = false;
            let mut stream_running = true;

            while let Some(command) = command_rx.blocking_recv() {
                let response = match command {
                    PlaybackCommand::SetUserPlaying { playing, response } => {
                        user_wants_playing = playing;
                        Some(response)
                    }
                    PlaybackCommand::SetSourceIdle(idle) => {
                        source_idle = idle;
                        None
                    }
                    PlaybackCommand::Shutdown => break,
                };

                let should_run = user_wants_playing && !source_idle;
                let transition = if should_run && !stream_running {
                    reset_requested.store(true, Ordering::Release);
                    render_enabled.store(true, Ordering::Release);
                    tracing::info!("[Playback] Starting output stream");
                    PlaybackOutput::start(&mut stream).inspect(|_| stream_running = true)
                } else if !should_run && stream_running {
                    render_enabled.store(false, Ordering::Release);
                    reset_requested.store(true, Ordering::Release);
                    tracing::info!(
                        source_idle,
                        user_wants_playing,
                        "[Playback] Pausing and flushing output stream"
                    );
                    PlaybackOutput::pause(&mut stream).inspect(|_| stream_running = false)
                } else {
                    Ok(())
                };

                match transition {
                    Ok(()) => {
                        if let Some(response) = response {
                            let _ = response.send(Ok(()));
                        }
                    }
                    Err(error) => {
                        render_enabled.store(false, Ordering::Release);
                        let message = error.to_string();
                        if let Some(response) = response {
                            let _ = response.send(Err(message.clone()));
                        }
                        let _ = error_tx.blocking_send(message);
                        break;
                    }
                }
            }

            render_enabled.store(false, Ordering::Release);
        })
    }
}
