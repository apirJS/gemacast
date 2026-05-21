/// Centralized port allocation for all GemaCast network services.
///
/// Each port serves a single, well-defined purpose to avoid multiplexing
/// concerns and simplify firewall configuration.
pub struct Ports;

impl Ports {
    /// UDP broadcast port for presence announcements (PC -> network).
    /// Carries only `Presence` and `Probe` messages.
    pub const DISCOVERY: u16 = 55555;

    /// HTTP port for control handshakes (mobile ↔ PC via Axum REST).
    /// Carries `Connect`, `Disconnect`, `GetSources`, `SourceList`,
    /// `ChangeSource`, and `Probe` requests/responses.
    pub const CONTROL: u16 = 55559;

    /// UDP port for real-time audio streaming (PC -> mobile).
    pub const AUDIO_UDP: u16 = 55556;

    /// TCP port for ADB-tunneled audio (PC -> mobile via `adb reverse`).
    /// Uses length-prefixed framing via [`TcpAudioFramer`].
    pub const ADB_AUDIO_TCP: u16 = 55557;

    /// TCP port for ADB-tunneled discovery (PC <-> mobile via `adb reverse`).
    /// Carries newline-delimited JSON `ControlMessage` payloads.
    pub const ADB_DISCOVERY_TCP: u16 = 55558;
}
