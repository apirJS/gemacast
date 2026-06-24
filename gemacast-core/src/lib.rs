// Hexagonal architecture layers
pub mod ports;      // Port trait definitions (hexagonal boundaries)
pub mod adapters;   // Production adapter implementations
pub mod domain;     // Domain facade (pure types, errors, audio constants)

// Existing modules (some now re-export from domain/adapters)
pub mod audio;
pub mod control;
pub mod discovery;
pub mod error;      // Re-exports from domain::error
pub mod jitter;
pub mod network;
pub mod stream;
pub mod types;      // Re-exports from domain::types

// Testing infrastructure
#[cfg(test)]
pub mod testing;
