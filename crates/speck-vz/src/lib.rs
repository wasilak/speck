#[cfg(not(all(target_arch = "aarch64", target_os = "macos")))]
compile_error!(
    "speck-vz targets aarch64-apple-darwin only; Virtualization.framework is macOS-only."
);

pub mod config;
mod delegate;
pub mod error;
pub mod guest;
mod vm_thread;
mod vsock;

pub use config::{GuestConfig, PortMapConfig};
pub use error::{Error, Result};
pub use guest::Guest;
pub use speck_core::{EngineEvent, EventSink, VmState};
pub use vsock::VzSocket;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
