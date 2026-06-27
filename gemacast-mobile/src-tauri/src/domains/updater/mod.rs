//! Auto-update checker, downloader, and APK installer (Android).
//!
//! Provides Tauri commands for checking the GitHub release manifest, downloading
//! the latest APK, and triggering the Android system installer via JNI.

pub mod commands;
pub mod install;
