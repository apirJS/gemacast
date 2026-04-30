use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum GemaCastError {
    #[error("{0}")]
    Protocol(#[from] ProtocolError),

    #[error("{0}")]
    AudioCapture(#[from] AudioCaptureError),

    #[error("{0}")]
    Network(#[from] NetworkError),

    #[error("{0}")]
    Control(#[from] ControlError),
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

    #[cfg(target_os = "windows")]
    #[error("Windows API Error: {0}")]
    WindowsApiError(#[from] windows::core::Error),

    #[error("per-process audio capture is not available on this platform")]
    ProcessCaptureUnavailable,

    #[error("process with PID {0} not found or not producing audio")]
    ProcessNotFound(u32),

    #[error("failed to create capture instance for source: {0}")]
    CaptureInstanceFailed(String),

    #[error("capture pool is full (max {max} concurrent captures)")]
    CapturePoolExhausted { max: usize },
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

    #[error("failed to configure socket reuse address")]
    SetReuseAddressFailed(#[source] std::io::Error),

    #[error("failed to configure socket reuse port")]
    SetReusePortFailed(#[source] std::io::Error),

    #[error("failed to configure socket type of service (TOS)")]
    SetTosFailed(#[source] std::io::Error),

    #[error("failed to set socket read timeout")]
    SetReadTimeoutFailed(#[source] std::io::Error),

    #[error("failed to clone socket")]
    SocketCloneFailed(#[source] std::io::Error),

    #[error("failed to enable broadcast feature")]
    EnableBroadcastFailed(#[source] std::io::Error),

    #[error("Failed to serialize discovery payload")]
    Serialization(#[from] serde_json::Error),

    #[error("failed to connect TCP stream to {addr}")]
    TcpConnectFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to bind TCP discovery spigot on {addr}")]
    TcpSpigotBindFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to bind TCP audio framer on {addr}")]
    TcpFramerBindFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("no active connection for device {0}")]
    DeviceNotConnected(String),
}

#[derive(ThisError, Debug)]
pub enum ControlError {
    #[error("failed to serialize control message")]
    Serialization(#[from] serde_json::Error),

    #[error("failed to send control message to {addr}: {source}")]
    SendFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("sender rejected the request: {reason}")]
    Rejected { reason: String },
}
