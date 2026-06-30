//! Crash logging — captures panics and writes them to disk.
//!
//! Log directory: `<data_dir>/gemacast/logs/`
//!
//! - **Windows**: `C:\Users\<user>\AppData\Roaming\gemacast\logs\`
//! - **Linux**:   `~/.local/share/gemacast/logs/`
//! - **macOS**:   `~/Library/Application Support/gemacast/logs/`
//!
//! Each panic produces a file named `crash-<ISO8601>.log` containing the
//! panic message, source location, and a captured backtrace.

use std::io::Write;
use std::path::PathBuf;

/// Maximum number of crash log files to keep.
const MAX_CRASH_LOGS: usize = 50;

/// Maximum age of crash log files in seconds (30 days).
const MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;

/// Returns the directory where crash logs are stored.
fn logs_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gemacast")
        .join("logs")
}

/// Install a custom panic hook that writes crash details to a log file.
///
/// Must be called as early as possible in `main()` — before any other
/// initialization that might panic.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        // Best-effort: write the crash log, but never panic inside the hook.
        let _ = write_crash_log(info);

        // Chain to the default hook so stderr still gets the message.
        default_hook(info);
    }));
}

/// Write panic information to a timestamped log file.
fn write_crash_log(info: &std::panic::PanicHookInfo<'_>) -> std::io::Result<()> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir)?;

    // Generate a filesystem-safe timestamp: 2026-06-30T12-15-00
    let now = std::time::SystemTime::now();
    let since_epoch = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since_epoch.as_secs();

    // Simple UTC breakdown (no chrono dependency needed)
    let (year, month, day, hour, min, sec) = epoch_to_utc(secs);
    let timestamp = format!(
        "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}",
        year, month, day, hour, min, sec
    );

    let filename = format!("crash-{timestamp}.log");
    let path = dir.join(filename);

    let mut file = std::fs::File::create(&path)?;

    // -- Header --
    writeln!(file, "=== GEMACAST CRASH LOG ===")?;
    writeln!(file, "Timestamp: {timestamp} UTC")?;
    writeln!(file)?;

    // -- Panic message --
    let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    };
    writeln!(file, "Panic: {payload}")?;

    // -- Location --
    if let Some(loc) = info.location() {
        writeln!(
            file,
            "Location: {}:{}:{}",
            loc.file(),
            loc.line(),
            loc.column()
        )?;
    }

    writeln!(file)?;

    // -- Backtrace --
    let backtrace = std::backtrace::Backtrace::force_capture();
    writeln!(file, "Backtrace:\n{backtrace}")?;

    Ok(())
}

/// Delete crash logs older than 30 days, or when there are more than 50 files.
///
/// Best-effort: failures are silently ignored.
pub fn cleanup_old_crash_logs() {
    let dir = logs_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let now = std::time::SystemTime::now();
    let mut crash_files: Vec<(PathBuf, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".log"))
        })
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), modified))
        })
        .collect();

    // Remove files older than MAX_AGE_SECS
    crash_files.retain(|(path, modified)| {
        if let Ok(age) = now.duration_since(*modified)
            && age.as_secs() > MAX_AGE_SECS
        {
            let _ = std::fs::remove_file(path);
            return false;
        }
        true
    });

    // If still too many, remove oldest first
    if crash_files.len() > MAX_CRASH_LOGS {
        crash_files.sort_by_key(|(_, modified)| *modified);
        let to_remove = crash_files.len() - MAX_CRASH_LOGS;
        for (path, _) in crash_files.iter().take(to_remove) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Convert a Unix epoch timestamp to (year, month, day, hour, minute, second) in UTC.
///
/// This is a minimal implementation that avoids pulling in a datetime crate.
fn epoch_to_utc(epoch: u64) -> (u64, u64, u64, u64, u64, u64) {
    let sec = epoch % 60;
    let min = (epoch / 60) % 60;
    let hour = (epoch / 3600) % 24;

    let mut days = epoch / 86400;

    // Days since 1970-01-01 to (year, remaining days-in-year)
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let month_days: [u64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 0u64;
    for (i, &md) in month_days.iter().enumerate() {
        if days < md {
            month = i as u64 + 1;
            break;
        }
        days -= md;
    }

    let day = days + 1;
    (year, month, day, hour, min, sec)
}

fn is_leap(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_to_utc_unix_epoch_should_be_1970_01_01() {
        let (y, m, d, h, min, s) = epoch_to_utc(0);
        assert_eq!((y, m, d, h, min, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_to_utc_known_date() {
        // 2026-06-30 12:00:00 UTC = 1782820800
        let (y, m, d, h, _, _) = epoch_to_utc(1782820800);
        assert_eq!(y, 2026);
        assert_eq!(m, 6);
        assert_eq!(d, 30);
        assert_eq!(h, 12);
    }

    #[test]
    fn logs_dir_should_end_with_gemacast_logs() {
        let dir = logs_dir();
        assert!(dir.ends_with("gemacast/logs") || dir.ends_with("gemacast\\logs"));
    }
}
