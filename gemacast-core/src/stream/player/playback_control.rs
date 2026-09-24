use crate::domain::error::{AudioError, GemaCastError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{mpsc, oneshot};

pub(crate) enum PlaybackCommand {
    SetUserPlaying {
        playing: bool,
        response: oneshot::Sender<Result<(), String>>,
    },
    SetSourceIdle(bool),
    Shutdown,
}

/// Serializes user and source-idle playback state changes.
#[derive(Clone)]
pub struct PlaybackControl {
    command_tx: mpsc::UnboundedSender<PlaybackCommand>,
    user_wants_playing: Arc<AtomicBool>,
}

impl PlaybackControl {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<PlaybackCommand>) {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        (
            Self {
                command_tx,
                user_wants_playing: Arc::new(AtomicBool::new(true)),
            },
            command_rx,
        )
    }

    async fn set_user_playing(&self, playing: bool) -> Result<(), GemaCastError> {
        let previous = self.user_wants_playing.swap(playing, Ordering::AcqRel);
        let (response_tx, response_rx) = oneshot::channel();
        if self
            .command_tx
            .send(PlaybackCommand::SetUserPlaying {
                playing,
                response: response_tx,
            })
            .is_err()
        {
            self.user_wants_playing.store(previous, Ordering::Release);
            return Err(AudioError::PlaybackControlUnavailable.into());
        }

        match response_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => {
                self.user_wants_playing.store(previous, Ordering::Release);
                Err(AudioError::PlaybackControlFailed(message).into())
            }
            Err(_) => {
                self.user_wants_playing.store(previous, Ordering::Release);
                Err(AudioError::PlaybackControlUnavailable.into())
            }
        }
    }

    pub async fn pause(&self) -> Result<(), GemaCastError> {
        self.set_user_playing(false).await
    }

    pub async fn resume(&self) -> Result<(), GemaCastError> {
        self.set_user_playing(true).await
    }

    pub(crate) fn user_wants_playing(&self) -> bool {
        self.user_wants_playing.load(Ordering::Acquire)
    }

    pub(crate) fn set_source_idle(&self, idle: bool) {
        let _ = self.command_tx.send(PlaybackCommand::SetSourceIdle(idle));
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.command_tx.send(PlaybackCommand::Shutdown);
    }
}
