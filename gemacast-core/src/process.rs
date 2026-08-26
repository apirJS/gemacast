//! One place to spawn a subprocess without flashing a console window.
//!
//! `gemacast-pc` is a GUI-subsystem binary on Windows, so it owns no console.
//! Every console child it spawns therefore allocates and *shows* a fresh
//! window, which the user sees as a black box popping up on screen.
//! `CREATE_NO_WINDOW` suppresses that.

use std::ffi::OsStr;

/// Run the child with no console at all
/// (`CREATE_NO_WINDOW`, <https://learn.microsoft.com/windows/win32/procthread/process-creation-flags>).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A [`std::process::Command`] that never pops a console window.
pub fn quiet_command<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    #[allow(unused_mut)] // Only Windows mutates it.
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A [`tokio::process::Command`] that never pops a console window.
///
/// `From<std::process::Command>` moves the std command whole, creation flags
/// included, so this inherits the suppression above rather than re-applying it.
pub fn quiet_tokio_command<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    tokio::process::Command::from(quiet_command(program))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A do-nothing binary that exists on every platform we build for.
    fn trivial() -> (&'static str, &'static [&'static str]) {
        if cfg!(windows) {
            ("cmd", &["/c", "exit"])
        } else {
            ("true", &[])
        }
    }

    // Note what these cannot prove: whether a window actually appears needs a
    // GUI-subsystem parent and a human watching. See the plan's verification
    // steps. What they do prove is that the flag we pass is a *valid* one — a
    // bogus creation flag makes the spawn itself fail with ERROR_INVALID_PARAMETER.

    #[test]
    fn quiet_command_produces_a_runnable_command() {
        let (program, args) = trivial();
        let status = quiet_command(program)
            .args(args)
            .status()
            .expect("should spawn");
        assert!(status.success());
    }

    #[tokio::test]
    async fn quiet_tokio_command_produces_a_runnable_command() {
        let (program, args) = trivial();
        let status = quiet_tokio_command(program)
            .args(args)
            .status()
            .await
            .expect("should spawn");
        assert!(status.success());
    }

    #[test]
    fn the_program_is_passed_through_unchanged() {
        assert_eq!(quiet_command("adb").get_program(), OsStr::new("adb"));
    }
}
