use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum GemaCastError {
    #[error("{0}")]
    Protocol(#[from] ProtocolError),

    #[error("{0}")]
    AudioCapture(#[from] AudioCaptureError),

    #[error("{0}")]
    Network(#[from] NetworkError),
}

#[derive(ThisError, Debug)]
pub enum ProtocolError {
    #[error("packet too short: expected at least {min} bytes, got {got}")]
    PacketTooShort { got: usize, min: usize },
}

#[derive(ThisError, Debug)]
pub enum AudioCaptureError {
    #[error("Audio host is not available")]
    HostUnavailable(#[source] cpal::HostUnavailable),

    #[error("no default output device available")]
    DefaultOutputDeviceUnavailable,

    #[error("failed to get default stream config from output device")]
    DefaultOutputStreamConfigUnavailable(#[from] cpal::DefaultStreamConfigError),

    #[error("failed to build input stream on output device")]
    FailedToBuildInputStream(#[source] cpal::BuildStreamError),

    #[error("failed to build output stream on output device")]
    FailedToBuildOutputStream(#[source] cpal::BuildStreamError),

    #[error("failed to play input stream")]
    FailedToPlayInputStream(#[source] cpal::PlayStreamError),

    #[error("failed to play output stream")]
    FailedToPlayOutputStream(#[source] cpal::PlayStreamError),

    #[error("failed to create Opus encoder")]
    OpusEncoderFailed(#[source] opus::Error),

    #[error("failed to create Opus decoder")]
    OpusDecoderFailed(#[source] opus::Error),

    #[error("Opus encoding failed")]
    OpusEncodeFailed(#[source] opus::Error),

    #[error("Opus decoding failed")]
    OpusDecodeFailed(#[source] opus::Error),

    #[error("cpal stream error")]
    StreamError(#[source] cpal::StreamError),
}

#[derive(ThisError, Debug)]
pub enum NetworkError {
    #[error("failed to bind UDP socket on {addr}")]
    BindFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to send UDP packet")]
    SendFailed(#[source] std::io::Error),

    #[error("failed to receive UDP packet")]
    RecvFailed(#[source] std::io::Error),

    #[error("failed to enable broadcast feature")]
    EnableBroadcastFailed(#[source] std::io::Error),

    #[error("Failed to serialize discovery payload")]
    Serialization(#[from] serde_json::Error),
}
