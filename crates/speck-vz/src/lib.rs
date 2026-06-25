// Public error types.
pub mod error;

// Guest configuration stub (full impl in Plan 02-02).
pub mod config;

// VM worker thread with dispatch queue infrastructure.
mod vm_thread;

// Re-export the public error types at the crate root.
pub use error::{Error, Result};

// Re-export core types that form part of the public API.
pub use speck_core::{EventSink, EngineEvent, VmState};

// ---------------------------------------------------------------------------
// Stubs preserved from Phase 1 — will evolve in later plans.
// ---------------------------------------------------------------------------

/// Return the crate version (from `Cargo.toml`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Accept (and discard) an event sink — placeholder for the future
/// observability pipeline built on top of `VmEventDispatcher`.
pub fn accepts_sink(_sink: &dyn speck_core::EventSink) {}

/// Re-export `VmThread` and `InternalState` for use by higher-level orchestration.
pub use vm_thread::{InternalState, VmThread};

