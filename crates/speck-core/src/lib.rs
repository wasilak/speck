#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

#[cfg(not(all(target_arch = "aarch64", target_os = "macos")))]
compile_error!("Speck targets aarch64-apple-darwin only; refusing non-Apple-Silicon-macOS target.");

pub mod config;
pub mod container;
pub mod events;
pub mod image;
pub mod network;
pub mod volume;

pub use config::NetworkConfig;
pub use container::{
    ContainerInspect, ContainerSpec, ContainerState, ContainerSummary, MountSpec, PortBinding,
};
pub use events::{EngineEvent, EventSink, LogLevel, NoopSink, VmState};
pub use image::{ImageInspect, ImageRef, ImageSummary, RegistryAuth};
pub use network::{NetworkCreateRequest, NetworkInspect, NetworkSummary};
pub use volume::{VolumeCreateRequest, VolumeInspect, VolumeSummary};
