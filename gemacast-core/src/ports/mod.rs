//! Port trait definitions — the hexagonal boundaries of gemacast-core.
//!
//! These traits define the contracts between the domain/orchestration layers
//! and external concerns (audio hardware, network I/O, UI notifications).
//! All orchestration structs are generic over these traits, enabling:
//!
//! - **Static dispatch** (default): zero vtable overhead via monomorphization.
//! - **Dynamic dispatch** (opt-in `dynamic-dispatch` feature): `Box<dyn Trait>` wrappers
//!   for plugin-style runtime polymorphism.
//!
//! # Production implementations
//!
//! See [`crate::adapters`] for concrete adapters used at runtime.

pub mod capture;
pub mod error_notifier;
pub mod process_lister;
pub mod transport;
