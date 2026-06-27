// Hexagonal architecture layers
pub mod adapters; // Production adapter implementations
pub mod domain;
pub mod ports; // Port trait definitions (hexagonal boundaries) // Domain facade (pure types, errors, audio constants)

// Existing modules (some now re-export from domain/adapters)
pub mod audio;
pub mod control;
pub mod discovery;

pub mod jitter;
pub mod network;
pub mod stream;
pub mod updater;

// Testing infrastructure
#[cfg(test)]
pub mod testing;
