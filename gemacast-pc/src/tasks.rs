//! Background tasks that power the PC sender.
//!
//! Each module contains one async task (or a small group of related tasks)
//! that runs in the background engine's `JoinSet`:
//!
//! - [`audio_engine`]: Runs the audio capture and streaming engine.
//! - [`command_handler`]: Processes [`AppCommand`](crate::events::AppCommand)s from the tray UI.
//! - [`control_dispatcher`]: Routes HTTP and UDP control commands to the appropriate handlers.
//! - [`device_watchdog`]: Evicts devices that stop sending probe heartbeats.
//! - [`udp_listener`]: Receives presence/probe messages on the UDP discovery port.

pub mod audio_engine;
pub mod command_handler;
pub mod control_dispatcher;
pub mod device_watchdog;
pub mod udp_listener;
pub mod updater;
