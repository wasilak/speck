#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

#[cfg(not(all(target_arch = "aarch64", target_os = "macos")))]
compile_error!("Speck targets aarch64-apple-darwin only; refusing non-Apple-Silicon-macOS target.");

pub mod config;
pub mod events;

pub use config::NetworkConfig;
pub use events::{EngineEvent, EventSink, LogLevel, NoopSink, VmState};
