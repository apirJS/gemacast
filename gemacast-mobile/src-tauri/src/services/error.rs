#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[cfg(not(target_os = "android"))]
    #[error("{operation} is not supported on this platform")]
    UnsupportedPlatform { operation: &'static str },

    #[error("platform bridge failed: {0}")]
    PlatformBridge(String),

    #[error("could not {operation}: {source}")]
    FileSystem {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
}

pub type ServiceResult<T> = Result<T, ServiceError>;
