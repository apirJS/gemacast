use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum GemaCastError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),

    #[error(transparent)]
    Audio(#[from] AudioError),

    #[error(transparent)]
    Network(#[from] NetworkError),

    #[error(transparent)]
    Control(#[from] ControlError),
}

#[derive(ThisError, Debug)]
pub enum ProtocolError {
    #[error("packet too short: expected at least {min} bytes, got {got}")]
    PacketTooShort { got: usize, min: usize },
}

#[derive(Debug, Clone, Copy)]
pub enum StreamDirection {
    Input,
    Output,
}

impl std::fmt::Display for StreamDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input => write!(f, "input"),
            Self::Output => write!(f, "output"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CodecDirection {
    Encoder,
    Decoder,
}

impl std::fmt::Display for CodecDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encoder => write!(f, "encoder"),
            Self::Decoder => write!(f, "decoder"),
        }
    }
}

#[derive(ThisError, Debug)]
pub enum AudioError {
    #[error("audio host is not available")]
    HostUnavailable(#[source] cpal::HostUnavailable),

    #[error("no default output device available")]
    NoOutputDevice,

    #[error("failed to get default stream config from output device")]
    StreamConfigUnavailable(#[from] cpal::DefaultStreamConfigError),

    #[error("failed to build {direction} stream on output device")]
    BuildStreamFailed {
        direction: StreamDirection,
        #[source]
        source: cpal::BuildStreamError,
    },

    #[error("failed to play {direction} stream")]
    PlayStreamFailed {
        direction: StreamDirection,
        #[source]
        source: cpal::PlayStreamError,
    },

    #[error("cpal stream error")]
    StreamError(#[source] cpal::StreamError),

    #[error("failed to create Opus {direction}")]
    OpusInitFailed {
        direction: CodecDirection,
        #[source]
        source: opus::Error,
    },

    #[error("Opus {direction} failed")]
    OpusCodecFailed {
        direction: CodecDirection,
        #[source]
        source: opus::Error,
    },

    #[cfg(target_os = "windows")]
    #[error("Windows API error: {0}")]
    WindowsApi(#[from] windows::core::Error),

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
    #[error("failed to bind socket on {addr}")]
    SocketBindFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to configure socket option: {option}")]
    SocketOptionFailed {
        option: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to send packet")]
    SendFailed(#[source] std::io::Error),

    #[error("failed to receive packet")]
    RecvFailed(#[source] std::io::Error),

    #[error("failed to clone socket")]
    SocketCloneFailed(#[source] std::io::Error),

    #[error("failed to enable broadcast")]
    EnableBroadcastFailed(#[source] std::io::Error),

    #[error("failed to serialize discovery payload")]
    Serialization(#[from] serde_json::Error),

    #[error("failed to connect TCP stream to {addr}")]
    TcpConnectFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("connection lost or sender stopped transmitting")]
    ConnectionLost,

    #[error("no active connection for device {0}")]
    DeviceNotConnected(String),

    #[error("failed to register mDNS service: {0}")]
    MdnsRegisterFailed(#[source] mdns_sd::Error),
}

#[derive(ThisError, Debug)]
pub enum ControlError {
    #[error("failed to serialize control message")]
    Serialization(#[from] serde_json::Error),

    #[error("failed to send control message to {addr}")]
    SendFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("sender rejected the request: {reason}")]
    Rejected { reason: String },

    #[error("failed to start control server")]
    ServerStartFailed(#[source] std::io::Error),

    #[error("HTTP request failed: {0}")]
    HttpRequestFailed(String),

    #[error("WebSocket connection failed: {reason}")]
    WebSocketFailed { reason: String },
}
