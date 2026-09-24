use std::time::Duration;

use crate::control::tls::client_config;
use crate::domain::error::{ControlError, GemaCastError};

pub(crate) fn request_error(error: reqwest::Error) -> GemaCastError {
    ControlError::HttpRequestFailed(format_error_chain(&error)).into()
}

pub(crate) fn build_https_client(
    timeout: Duration,
    expected_fingerprint: Option<&str>,
) -> Result<reqwest::Client, String> {
    let tls_config = client_config(expected_fingerprint)?;
    reqwest::Client::builder()
        .timeout(timeout)
        // The control endpoint is a direct peer on the local network. Never
        // route it through HTTP(S)_PROXY or an Android system proxy: that can
        // turn a reachable private IP into a silent ten-second timeout before
        // the PC's listener sees any TCP connection.
        .no_proxy()
        .tls_info(true)
        .https_only(true)
        .pool_max_idle_per_host(0)
        .use_preconfigured_tls(tls_config)
        .build()
        .map_err(|error| format!("failed to configure HTTPS control client: {error}"))
}

fn format_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        let detail = error.to_string();
        if !detail.is_empty() && !message.contains(&detail) {
            message.push_str(": ");
            message.push_str(&detail);
        }
        source = error.source();
    }
    message
}
