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

    #[error("audio resampling failed: {0}")]
    ResampleFailed(String),

    #[error("source is not actively subscribed")]
    SourceNotSubscribed,
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

#[cfg(test)]
mod tests {
    use super::*;

    mod protocol_error {
        use super::*;

        #[test]
        fn packet_too_short_should_display_expected_and_actual_sizes() {
            let err = ProtocolError::PacketTooShort { got: 3, min: 9 };
            let msg = err.to_string();
            assert!(
                msg.contains("at least 9") && msg.contains("got 3"),
                "Expected sizes in message, got: {msg}"
            );
        }

        #[test]
        fn should_convert_into_gemacast_error_via_from() {
            let err = ProtocolError::PacketTooShort { got: 0, min: 9 };
            let outer: GemaCastError = err.into();
            assert!(
                matches!(outer, GemaCastError::Protocol(_)),
                "Expected GemaCastError::Protocol, got: {outer:?}"
            );
        }
    }

    mod network_error {
        use super::*;

        #[test]
        fn connection_lost_should_display_descriptive_message() {
            let err = NetworkError::ConnectionLost;
            assert!(
                err.to_string().contains("connection lost"),
                "Expected 'connection lost' in: {}",
                err
            );
        }

        #[test]
        fn device_not_connected_should_include_device_id() {
            let err = NetworkError::DeviceNotConnected("phone_42".to_string());
            assert!(
                err.to_string().contains("phone_42"),
                "Expected device id in: {}",
                err
            );
        }

        #[test]
        fn should_convert_into_gemacast_error_via_from() {
            let err = NetworkError::ConnectionLost;
            let outer: GemaCastError = err.into();
            assert!(
                matches!(outer, GemaCastError::Network(_)),
                "Expected GemaCastError::Network, got: {outer:?}"
            );
        }
    }

    mod audio_error {
        use super::*;

        #[test]
        fn no_output_device_should_display_descriptive_message() {
            let err = AudioError::NoOutputDevice;
            assert!(
                err.to_string().contains("no default output device"),
                "Expected 'no default output device' in: {}",
                err
            );
        }
    }

    mod control_error {
        use super::*;

        #[test]
        fn timeout_should_include_duration_in_display() {
            let err = ControlError::Timeout { timeout_ms: 3000 };
            assert!(
                err.to_string().contains("3000"),
                "Expected timeout value in: {}",
                err
            );
        }

        #[test]
        fn should_convert_into_gemacast_error_via_from() {
            let err = ControlError::Timeout { timeout_ms: 500 };
            let outer: GemaCastError = err.into();
            assert!(
                matches!(outer, GemaCastError::Control(_)),
                "Expected GemaCastError::Control, got: {outer:?}"
            );
        }
    }

    mod direction_display {
        use super::*;

        #[test]
        fn stream_direction_input_should_display_lowercase() {
            assert_eq!(StreamDirection::Input.to_string(), "input");
        }

        #[test]
        fn stream_direction_output_should_display_lowercase() {
            assert_eq!(StreamDirection::Output.to_string(), "output");
        }

        #[test]
        fn codec_direction_encoder_should_display_lowercase() {
            assert_eq!(CodecDirection::Encoder.to_string(), "encoder");
        }

        #[test]
        fn codec_direction_decoder_should_display_lowercase() {
            assert_eq!(CodecDirection::Decoder.to_string(), "decoder");
        }
    }
}
