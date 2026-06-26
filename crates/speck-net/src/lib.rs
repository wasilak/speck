pub mod config;
pub mod error;

pub use error::{Error, Result};

pub struct SpeckNet;

impl SpeckNet {
    pub fn new(_config: config::NetworkConfig) -> Self {
        SpeckNet
    }

    pub fn spawn(self, _fd: std::os::unix::io::RawFd) -> Self {
        SpeckNet
    }
}
