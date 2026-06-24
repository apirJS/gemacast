//! Port: OS process enumeration.
//!
//! Decouples the HTTP control server from platform-specific process listing APIs.
//! On Windows this uses WASAPI session enumeration + Toolhelp32; on other
//! platforms it returns an empty list.

use crate::types::ProcessInfo;

/// Lists capturable audio processes on the host OS.
///
/// # Production
///
/// [`crate::adapters::process_lister::DefaultProcessLister`] — Windows WASAPI
/// session enumeration + Toolhelp32 snapshot.
///
/// # Testing
///
/// Mock implementations return canned process lists.
pub trait ProcessLister: Send + Sync {
    /// Return all processes currently producing audio.
    ///
    /// The list is filtered to exclude system/infrastructure processes.
    fn list_processes(&self) -> Vec<ProcessInfo>;
}
