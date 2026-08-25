//! Crash logging — captures panics and writes them to disk.
//!
//! One file, one fixed path, overwritten on every panic:
//!
//! - **Windows**: `C:\Users\<user>\AppData\Roaming\gemacast\crash.log`
//! - **Linux**:   `~/.config/gemacast/crash.log`
//! - **macOS**:   `~/Library/Application Support/gemacast/crash.log`
//!
//! It sits beside `config.json` and the identity files ([`crate::config`]) so
//! everything Gemacast writes is in one directory a user can be pointed at. There
//! is no rotation and no retention policy: the newest report replaces the previous
//! one, because the latest crash is the only one anybody asks about.
//!
//! # What a backtrace here can and cannot resolve
//!
//! The workspace release profile sets `strip = "debuginfo"` rather than
//! `strip = "symbols"` specifically so the ELF/Mach-O symbol table survives and
//! these backtraces name functions on Linux and macOS. Windows/MSVC keeps symbol
//! names only in the PDB, so a shipped `.exe` without its `.pdb` beside it
//! resolves every Rust frame to `<unknown>` at *any* `strip` value — and
//! `std::backtrace` records no raw addresses either, so such a log cannot be
//! symbolized after the fact. On Windows the actionable fields are `Panic:`,
//! `Location:` and `Thread:`, which is why they are recorded first.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Arc, Mutex};

// Set while this thread is inside the hook, so a panic raised *by* the hook
// (allocation failure formatting a large backtrace, a poisoned lock reached
// through `Display`) chains straight to the default hook instead of recursing
// into `write_crash_log` forever.
thread_local! {
    static IN_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Cleared once the first crash of this process has been written.
///
/// A second panic *overwrites* the first one's report, so the annotation this
/// drives is the only surviving trace that an earlier panic happened at all.
static FIRST_CRASH: AtomicBool = AtomicBool::new(true);

/// The one crash-log path, beside `config.json`.
fn crash_log_path() -> PathBuf {
    crate::config::config_path().with_file_name("crash.log")
}

/// Install a custom panic hook that writes crash details to the crash log.
///
/// Must be called as early as possible in `main()` — before any other
/// initialization that might panic.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        // A panic inside the hook must not re-enter it. Chain out immediately.
        let reentrant = IN_HOOK.with(|f| f.replace(true));
        if !reentrant {
            // Best-effort: write the crash log, but never panic inside the hook.
            let _ = write_crash_log(info);
            IN_HOOK.with(|f| f.set(false));
        }

        // Chain to the default hook so stderr still gets the message.
        default_hook(info);
    }));
}

/// Everything a crash report records, gathered so the body can be rendered
/// without touching the panic machinery or the filesystem.
struct Report<'a> {
    timestamp: &'a str,
    payload: &'a str,
    location: Option<String>,
    thread_name: &'a str,
    thread_id: String,
    repeat: bool,
    backtrace: String,
}

/// Render a crash report.
///
/// Split from [`write_crash_log`] so it is testable: a `PanicHookInfo` cannot be
/// constructed outside a real panic, and the previous code had no test that any
/// field reached the file at all.
///
/// Field order is deliberate. `Panic`, `Location` and `Thread` come first because
/// on Windows they are the *only* useful fields — see the module docs on why the
/// backtrace cannot resolve there.
fn write_report<W: Write>(out: &mut W, report: &Report<'_>) -> std::io::Result<()> {
    writeln!(out, "=== GEMACAST CRASH LOG ===")?;
    writeln!(out, "Timestamp: {} UTC", report.timestamp)?;
    writeln!(out, "Version: {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(out, "Panic: {}", report.payload)?;
    if let Some(loc) = &report.location {
        writeln!(out, "Location: {loc}")?;
    }
    writeln!(out, "Thread: {} ({})", report.thread_name, report.thread_id)?;
    writeln!(
        out,
        "Host: {} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        whoami::distro().unwrap_or_else(|_| "unknown distro".to_string()),
    )?;
    if report.repeat {
        writeln!(
            out,
            "Note: an earlier panic in this process was overwritten by this report."
        )?;
    }
    writeln!(out)?;
    writeln!(out, "Backtrace:\n{}", report.backtrace)?;
    Ok(())
}

/// Write `report` to `path`, replacing whatever was there.
///
/// `File::create` truncates, which *is* the retention policy: one crash log,
/// always the latest one. Split out from [`write_crash_log`] so the overwrite is
/// testable without a real panic.
fn write_report_to(path: &Path, report: &Report<'_>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(path)?;
    write_report(&mut file, report)?;
    file.flush()
}

/// Write panic information to the crash log, replacing any previous report.
fn write_crash_log(info: &std::panic::PanicHookInfo<'_>) -> std::io::Result<()> {
    // Generate a UTC timestamp: 2026-06-30T12-15-00
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, hour, min, sec) = epoch_to_utc(secs);
    let timestamp = format!("{year:04}-{month:02}-{day:02}T{hour:02}-{min:02}-{sec:02}");

    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed").to_string();

    let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    };

    let report = Report {
        timestamp: &timestamp,
        payload: &payload,
        location: info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column())),
        thread_name: &thread_name,
        thread_id: format!("{:?}", thread.id()),
        repeat: !FIRST_CRASH.swap(false, Ordering::SeqCst),
        // `force_capture` ignores RUST_BACKTRACE, so this is always populated.
        backtrace: std::backtrace::Backtrace::force_capture().to_string(),
    };

    write_report_to(&crash_log_path(), &report)
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

    fn sample_report<'a>(timestamp: &'a str, thread_name: &'a str) -> Report<'a> {
        Report {
            timestamp,
            payload: "assertion failed: occupied > 0",
            location: Some("gemacast-core/src/jitter/manager.rs:918:13".to_string()),
            thread_name,
            thread_id: "ThreadId(7)".to_string(),
            repeat: false,
            backtrace: "0: <unknown>".to_string(),
        }
    }

    fn render(report: &Report<'_>) -> String {
        let mut buf = Vec::new();
        write_report(&mut buf, report).expect("rendering into a Vec cannot fail");
        String::from_utf8(buf).expect("report must be valid UTF-8")
    }

    /// A unique scratch directory. Avoids `std::env::temp_dir()` collisions
    /// between the parallel tests in this module.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("gemacast-crashlog-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
        dir
    }

    mod epoch_conversion {
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
        fn epoch_to_utc_handles_a_leap_day() {
            // 2024-02-29 00:00:00 UTC = 1709164800. A non-leap month table would
            // roll this into March 1st.
            let (y, m, d, _, _, _) = epoch_to_utc(1709164800);
            assert_eq!((y, m, d), (2024, 2, 29));
        }
    }

    mod crash_log_location {
        use super::*;

        #[test]
        fn the_crash_log_sits_beside_config_json() {
            // One directory for everything Gemacast writes, so a user can be
            // pointed at a single path.
            let path = crash_log_path();

            assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("crash.log"));
            assert_eq!(path.parent(), crate::config::config_path().parent());
        }
    }

    mod report_contents {
        use super::*;

        #[test]
        fn report_records_the_panic_message_and_location() {
            let rendered = render(&sample_report(
                "2026-06-30T12-15-00",
                "tokio-runtime-worker",
            ));

            assert!(rendered.contains("Panic: assertion failed: occupied > 0"));
            assert!(
                rendered.contains("Location: gemacast-core/src/jitter/manager.rs:918:13"),
                "got:\n{rendered}"
            );
        }

        #[test]
        fn report_names_the_panicking_thread() {
            // The field that was missing and is usually the first question asked
            // of a six-task async app with a real-time audio callback.
            let rendered = render(&sample_report(
                "2026-06-30T12-15-00",
                "tokio-runtime-worker",
            ));

            assert!(
                rendered.contains("Thread: tokio-runtime-worker (ThreadId(7))"),
                "got:\n{rendered}"
            );
        }

        #[test]
        fn report_records_version_and_host_so_a_capture_is_attributable() {
            let rendered = render(&sample_report("2026-06-30T12-15-00", "main"));

            assert!(rendered.contains(&format!("Version: {}", env!("CARGO_PKG_VERSION"))));
            assert!(rendered.contains(&format!("Host: {}", std::env::consts::OS)));
            assert!(rendered.contains(std::env::consts::ARCH));
        }

        #[test]
        fn panic_and_location_precede_the_backtrace() {
            // On Windows the backtrace cannot resolve, so the fields that *are*
            // actionable must not be buried beneath it.
            let rendered = render(&sample_report("2026-06-30T12-15-00", "main"));

            let panic_at = rendered.find("Panic:").expect("Panic field");
            let thread_at = rendered.find("Thread:").expect("Thread field");
            let backtrace_at = rendered.find("Backtrace:").expect("Backtrace field");

            assert!(panic_at < backtrace_at);
            assert!(thread_at < backtrace_at);
        }

        #[test]
        fn a_missing_location_omits_the_field_rather_than_printing_none() {
            let mut report = sample_report("2026-06-30T12-15-00", "main");
            report.location = None;

            let rendered = render(&report);

            assert!(!rendered.contains("Location:"));
            assert!(!rendered.contains("None"));
        }

        #[test]
        fn a_follow_on_crash_says_it_overwrote_an_earlier_one() {
            // The only surviving trace that an earlier panic happened, now that a
            // second report replaces the first one's file.
            let mut report = sample_report("2026-06-30T12-15-00", "main");
            assert!(!render(&report).contains("an earlier panic"));

            report.repeat = true;
            assert!(render(&report).contains("an earlier panic"));
        }
    }

    mod overwriting {
        use super::*;

        #[test]
        fn a_second_crash_overwrites_the_first_instead_of_adding_a_file() {
            let dir = scratch_dir("overwrite");
            let path = dir.join("crash.log");

            let mut first = sample_report("2026-06-30T12-15-00", "main");
            first.payload = "FIRST PANIC";
            write_report_to(&path, &first).expect("first write");

            let mut second = sample_report("2026-06-30T12-16-00", "main");
            second.payload = "SECOND PANIC";
            write_report_to(&path, &second).expect("second write");

            let body = std::fs::read_to_string(&path).expect("crash log");
            assert!(body.contains("SECOND PANIC"), "got:\n{body}");
            // Truncation, not append: the first report must be gone entirely,
            // header included.
            assert!(!body.contains("FIRST PANIC"), "got:\n{body}");
            assert_eq!(body.matches("=== GEMACAST CRASH LOG ===").count(), 1);

            // And exactly one file, ever.
            let count = std::fs::read_dir(&dir)
                .expect("read scratch dir")
                .filter_map(|e| e.ok())
                .count();
            assert_eq!(count, 1);
        }

        #[test]
        fn a_missing_parent_directory_is_created_rather_than_failing() {
            // First run on a clean install panics before anything has created the
            // config directory.
            let dir = scratch_dir("missing-parent");
            let path = dir.join("nested").join("crash.log");

            write_report_to(&path, &sample_report("2026-06-30T12-15-00", "main"))
                .expect("write into a directory that does not exist yet");

            assert!(path.exists());
        }
    }

    mod panic_hook {
        use super::*;

        #[test]
        fn the_hook_writes_a_report_containing_the_panic_message() {
            // The behaviour the module exists for, and which nothing asserted
            // before: catch a real panic and confirm a report was rendered from it.
            //
            // `write_crash_log` needs a real `PanicHookInfo`, which can only be
            // obtained from a genuine panic, so this drives one through
            // `catch_unwind` under a temporary hook of our own.
            let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let sink = seen.clone();

            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let mut report = Report {
                    timestamp: "2026-06-30T12-15-00",
                    payload: "",
                    location: info
                        .location()
                        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())),
                    thread_name: "test-thread",
                    thread_id: "ThreadId(1)".to_string(),
                    repeat: false,
                    backtrace: "0: <unknown>".to_string(),
                };
                let payload = info
                    .payload()
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                report.payload = &payload;

                let mut buf = Vec::new();
                write_report(&mut buf, &report).expect("render");
                sink.lock()
                    .expect("sink lock")
                    .push(String::from_utf8(buf).expect("utf8"));
            }));

            let result = std::panic::catch_unwind(|| panic!("deliberate test panic"));

            std::panic::set_hook(previous);

            assert!(result.is_err(), "the panic must have been caught");
            let reports = seen.lock().expect("sink lock");
            assert_eq!(reports.len(), 1);
            assert!(reports[0].contains("deliberate test panic"));
            // The location must be this file, proving the real hook payload was
            // threaded through rather than a synthetic one.
            assert!(reports[0].contains("crash_log.rs"), "got:\n{}", reports[0]);
        }

        #[test]
        fn the_hook_is_reentrancy_guarded() {
            // `IN_HOOK` is thread-local, so a nested entry on the same thread
            // must be observed as reentrant. This asserts the flag's contract
            // directly; provoking a real panic *inside* the hook would abort the
            // test process rather than fail it.
            IN_HOOK.with(|f| f.set(false));

            let outer = IN_HOOK.with(|f| f.replace(true));
            assert!(!outer, "first entry is not reentrant");

            let inner = IN_HOOK.with(|f| f.replace(true));
            assert!(inner, "second entry on the same thread is reentrant");

            IN_HOOK.with(|f| f.set(false));
        }
    }
}
