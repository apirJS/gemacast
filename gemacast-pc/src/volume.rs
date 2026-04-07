/// Cross-platform system volume control.
///
/// Platform coverage:
///   - Windows  — IAudioEndpointVolume (Core Audio COM API)
///   - Linux    — PipeWire (wpctl) → PulseAudio (pactl) fallback
///   - macOS    — osascript (AppleScript) output volume
///   - Other    — no-op stub (always reports 100 % / unmuted)
pub trait SystemVolume {
    fn get_volume(&self) -> Result<f32, String>;
    fn set_volume(&self, level: f32) -> Result<(), String>;
    fn get_mute(&self) -> Result<bool, String>;
    fn set_mute(&self, muted: bool) -> Result<(), String>;
}

/// Return the best available volume controller for the current platform.
pub fn default_volume_controller() -> Box<dyn SystemVolume + Send + Sync> {
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsVolume)
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(LinuxVolume)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(MacVolume)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        Box::new(StubVolume)
    }
}

// ─── Windows ────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
pub struct WindowsVolume;

#[cfg(target_os = "windows")]
impl Default for WindowsVolume {
    fn default() -> Self {
        Self
    }
}

#[cfg(target_os = "windows")]
impl WindowsVolume {
    /// Acquire the default render-endpoint volume interface.
    ///
    /// COM is initialised on the calling thread via `CoInitializeEx`.  For
    /// calls originating from Tokio worker threads we use `COINIT_MULTITHREADED`
    /// which is safe as long as we never marshal the interface across threads
    /// (and we don't — the `Box<dyn SystemVolume>` is only used on the thread
    /// that created it, inside the broadcast factory closure).
    fn get_endpoint() -> Result<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume, String> {
        use windows::Win32::Media::Audio::{
            eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() 
                && hr != windows::Win32::Foundation::S_FALSE 
                && hr != windows::Win32::Foundation::RPC_E_CHANGED_MODE 
            {
                let err_msg = format!("CoInitializeEx failed: {hr:?}");
                eprintln!("{err_msg}");
                return Err(err_msg);
            }

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| {
                        eprintln!("CoCreateInstance(IMMDeviceEnumerator): {e}");
                        format!("CoCreateInstance(IMMDeviceEnumerator): {e}")
                    })?;

            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .map_err(|e| {
                    eprintln!("GetDefaultAudioEndpoint: {e}");
                    format!("GetDefaultAudioEndpoint: {e}")
                })?;

            device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| {
                    eprintln!("Activate(IAudioEndpointVolume): {e}");
                    format!("Activate(IAudioEndpointVolume): {e}")
                })
        }
    }
}

#[cfg(target_os = "windows")]
impl SystemVolume for WindowsVolume {
    fn get_volume(&self) -> Result<f32, String> {
        let ep = Self::get_endpoint()?;
        // Scalar range is always [0.0, 1.0].
        unsafe {
            ep.GetMasterVolumeLevelScalar()
                .map_err(|e| format!("GetMasterVolumeLevelScalar: {e}"))
        }
    }

    fn set_volume(&self, level: f32) -> Result<(), String> {
        let ep = Self::get_endpoint()?;
        // Clamp strictly to [0.0, 1.0] — the API rejects values outside that range.
        let clamped = level.clamp(0.0, 1.0);
        unsafe {
            ep.SetMasterVolumeLevelScalar(clamped, std::ptr::null())
                .map_err(|e| format!("SetMasterVolumeLevelScalar: {e}"))
        }
    }

    fn get_mute(&self) -> Result<bool, String> {
        let ep = Self::get_endpoint()?;
        unsafe {
            ep.GetMute()
                .map(|b| b.as_bool())
                .map_err(|e| format!("GetMute: {e}"))
        }
    }

    fn set_mute(&self, muted: bool) -> Result<(), String> {
        let ep = Self::get_endpoint()?;
        unsafe {
            ep.SetMute(muted, std::ptr::null())
                .map_err(|e| format!("SetMute: {e}"))
        }
    }
}

// ─── Linux ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub struct LinuxVolume;

/// Try to run `cmd` with `args`, return stdout on success (exit 0), else None.
#[cfg(target_os = "linux")]
fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Same as `run_cmd` but we only care whether the command succeeded.
#[cfg(target_os = "linux")]
fn run_cmd_ok(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
impl SystemVolume for LinuxVolume {
    fn get_volume(&self) -> Result<f32, String> {
        // ── PipeWire / WirePlumber ────────────────────────────────────────
        // `wpctl get-volume @DEFAULT_AUDIO_SINK@` → "Volume: 0.50\n"
        //                                       or "Volume: 0.50 [MUTED]\n"
        if let Some(stdout) = run_cmd("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]) {
            // The numeric token is always the second whitespace-separated field.
            if let Some(vol_str) = stdout.split_whitespace().nth(1) {
                if let Ok(vol) = vol_str.parse::<f32>() {
                    // wpctl can return values > 1.0 (150 % boost), normalise.
                    return Ok(vol.clamp(0.0, 1.0));
                }
            }
        }

        // ── PulseAudio ────────────────────────────────────────────────────
        // `pactl get-sink-volume @DEFAULT_SINK@` →
        //   "Volume: front-left: 32768 /  50% / -18.06 dB,  front-right: …"
        if let Some(stdout) = run_cmd("pactl", &["get-sink-volume", "@DEFAULT_SINK@"]) {
            if let Some(pct_pos) = stdout.find('%') {
                // Walk backwards from '%' to find the start of the number.
                let before = &stdout[..pct_pos];
                if let Some(start) = before.rfind(|c: char| !c.is_ascii_digit()) {
                    if let Ok(pct) = before[start + 1..].trim().parse::<f32>() {
                        return Ok((pct / 100.0).clamp(0.0, 1.0));
                    }
                }
            }
        }

        Err("No supported audio system found (tried wpctl, pactl)".to_string())
    }

    fn set_volume(&self, level: f32) -> Result<(), String> {
        let clamped = level.clamp(0.0, 1.0);

        // wpctl uses decimal fractions (e.g. "0.50").
        if run_cmd_ok("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{clamped:.2}")]) {
            return Ok(());
        }

        // pactl uses percentage strings (e.g. "50%").
        if run_cmd_ok(
            "pactl",
            &["set-sink-volume", "@DEFAULT_SINK@", &format!("{:.0}%", clamped * 100.0)],
        ) {
            return Ok(());
        }

        Err("set_volume failed: neither wpctl nor pactl succeeded".to_string())
    }

    fn get_mute(&self) -> Result<bool, String> {
        // wpctl signals mute via "[MUTED]" suffix on get-volume output.
        if let Some(stdout) = run_cmd("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]) {
            return Ok(stdout.contains("[MUTED]"));
        }

        // `pactl get-sink-mute @DEFAULT_SINK@` → "Mute: yes\n" or "Mute: no\n"
        if let Some(stdout) = run_cmd("pactl", &["get-sink-mute", "@DEFAULT_SINK@"]) {
            // Robust: split on ':' and check the value side.
            let value = stdout.splitn(2, ':').nth(1).unwrap_or("").trim().to_lowercase();
            return Ok(value.starts_with("yes"));
        }

        Err("get_mute failed: neither wpctl nor pactl succeeded".to_string())
    }

    fn set_mute(&self, muted: bool) -> Result<(), String> {
        // wpctl: "1" = mute, "0" = unmute.
        let wpctl_arg = if muted { "1" } else { "0" };
        if run_cmd_ok("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", wpctl_arg]) {
            return Ok(());
        }

        // pactl: "1" = mute, "0" = unmute.
        let pactl_arg = if muted { "1" } else { "0" };
        if run_cmd_ok("pactl", &["set-sink-mute", "@DEFAULT_SINK@", pactl_arg]) {
            return Ok(());
        }

        Err("set_mute failed: neither wpctl nor pactl succeeded".to_string())
    }
}

// ─── macOS ──────────────────────────────────────────────────────────────────

/// macOS volume control via `osascript` (AppleScript).
/// Requires no additional dependencies — `osascript` ships with every macOS.
#[cfg(target_os = "macos")]
pub struct MacVolume;

#[cfg(target_os = "macos")]
impl SystemVolume for MacVolume {
    fn get_volume(&self) -> Result<f32, String> {
        // Returns integer 0-100.
        let out = std::process::Command::new("osascript")
            .args(["-e", "output volume of (get volume settings)"])
            .output()
            .map_err(|e| format!("osascript get volume: {e}"))?;
        if !out.status.success() {
            return Err("osascript exited non-zero while reading volume".to_string());
        }
        let s = String::from_utf8_lossy(&out.stdout);
        s.trim()
            .parse::<f32>()
            .map(|v| v / 100.0)
            .map_err(|e| format!("Failed to parse osascript volume output: {e}"))
    }

    fn set_volume(&self, level: f32) -> Result<(), String> {
        let pct = (level.clamp(0.0, 1.0) * 100.0).round() as u8;
        let status = std::process::Command::new("osascript")
            .args(["-e", &format!("set volume output volume {pct}")])
            .status()
            .map_err(|e| format!("osascript set volume: {e}"))?;
        if status.success() { Ok(()) } else { Err("osascript exited non-zero while setting volume".to_string()) }
    }

    fn get_mute(&self) -> Result<bool, String> {
        let out = std::process::Command::new("osascript")
            .args(["-e", "output muted of (get volume settings)"])
            .output()
            .map_err(|e| format!("osascript get mute: {e}"))?;
        if !out.status.success() {
            return Err("osascript exited non-zero while reading mute".to_string());
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
        Ok(s == "true")
    }

    fn set_mute(&self, muted: bool) -> Result<(), String> {
        let arg = if muted { "true" } else { "false" };
        let status = std::process::Command::new("osascript")
            .args(["-e", &format!("set volume output muted {arg}")])
            .status()
            .map_err(|e| format!("osascript set mute: {e}"))?;
        if status.success() { Ok(()) } else { Err("osascript exited non-zero while setting mute".to_string()) }
    }
}

// ─── Stub (other platforms) ──────────────────────────────────────────────────

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub struct StubVolume;

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
impl SystemVolume for StubVolume {
    fn get_volume(&self) -> Result<f32, String> { Ok(1.0) }
    fn set_volume(&self, _level: f32) -> Result<(), String> { Ok(()) }
    fn get_mute(&self) -> Result<bool, String> { Ok(false) }
    fn set_mute(&self, _muted: bool) -> Result<(), String> { Ok(()) }
}
