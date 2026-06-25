use std::path::PathBuf;
use std::time::Duration;

/// Configuration for a micro-VM guest.
///
/// This is the primary input to [`Guest::start`](crate::Guest::start).
/// Defaults are sensible for testing; production use should set
/// `kernel_path`, `cpu_count` and `memory_size_bytes` explicitly.
#[derive(Debug, Clone)]
pub struct GuestConfig {
    /// Absolute path to the guest Linux kernel image.
    ///
    /// Must point to a valid arm64 Linux kernel (e.g. the Kata
    /// containers kernel). The path must exist at VM start time.
    pub kernel_path: PathBuf,

    /// Optional initial RAM disk (initrd).
    ///
    /// If set, the kernel will load this as the initial root
    /// filesystem before pivoting to the real root.
    pub initrd_path: Option<PathBuf>,

    /// Kernel command-line parameters.
    ///
    /// Passed to the kernel via `VZLinuxBootLoader.commandLine`.
    /// Defaults to `"console=hvc0 earlycon=pl011,0x3f8"` if empty.
    pub cmdline: String,

    /// Number of virtual CPUs (≥ 1).
    pub cpu_count: u64,

    /// Memory size in bytes (≥ 512 MiB, multiple of 1 MiB).
    pub memory_size_bytes: u64,

    /// How long to wait for a graceful stop before force-stopping.
    ///
    /// Defaults to 10 seconds.
    pub stop_timeout: Duration,

    /// Guest vsock port for the control-plane connection.
    ///
    /// The host connects to this port inside the guest to reach
    /// `vminitd` (or equivalent).  Defaults to 1234.
    pub vsock_port: u32,
}

impl Default for GuestConfig {
    fn default() -> Self {
        Self {
            kernel_path: PathBuf::new(),
            initrd_path: None,
            cmdline: String::new(),
            cpu_count: 1,
            memory_size_bytes: 512 * 1024 * 1024,
            stop_timeout: Duration::from_secs(10),
            vsock_port: 1234,
        }
    }
}

impl GuestConfig {
    /// Create a new builder for `GuestConfig`.
    pub fn builder() -> GuestConfigBuilder {
        GuestConfigBuilder::default()
    }

    /// Validate the configuration against known constraints.
    ///
    /// Checks:
    /// - `kernel_path` exists on disk
    /// - `initrd_path` exists if set
    /// - `cpu_count` ≥ 1
    /// - `memory_size_bytes` ≥ 512 MiB (the Virtualization.framework minimum)
    pub fn validate(&self) -> Result<(), String> {
        if !self.kernel_path.exists() {
            return Err(format!("kernel not found: {}", self.kernel_path.display()));
        }
        if self.cpu_count == 0 {
            return Err("cpu_count must be at least 1".into());
        }
        if self.memory_size_bytes < 512 * 1024 * 1024 {
            return Err("memory_size_bytes must be at least 512 MiB".into());
        }
        if let Some(ref initrd) = self.initrd_path
            && !initrd.exists()
        {
            return Err(format!("initrd not found: {}", initrd.display()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for [`GuestConfig`].
///
/// ```
/// use speck_vz::GuestConfig;
/// use std::time::Duration;
///
/// let config = GuestConfig::builder()
///     .kernel_path("/tmp/Image")
///     .cpu_count(2)
///     .memory_size_bytes(1024 * 1024 * 1024)
///     .cmdline("console=hvc0")
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct GuestConfigBuilder {
    kernel_path: Option<PathBuf>,
    initrd_path: Option<PathBuf>,
    cmdline: String,
    cpu_count: u64,
    memory_size_bytes: u64,
    stop_timeout: Duration,
    vsock_port: u32,
}

impl Default for GuestConfigBuilder {
    fn default() -> Self {
        Self {
            kernel_path: None,
            initrd_path: None,
            cmdline: String::new(),
            cpu_count: 1,
            memory_size_bytes: 512 * 1024 * 1024,
            stop_timeout: Duration::from_secs(10),
            vsock_port: 1234,
        }
    }
}

impl GuestConfigBuilder {
    /// Set the path to the kernel image (required).
    pub fn kernel_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.kernel_path = Some(path.into());
        self
    }

    /// Set an optional initrd image.
    pub fn initrd_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.initrd_path = Some(path.into());
        self
    }

    /// Set the kernel command line.
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.cmdline = cmdline.into();
        self
    }

    /// Set the number of virtual CPUs (≥ 1).
    pub fn cpu_count(mut self, count: u64) -> Self {
        self.cpu_count = count;
        self
    }

    /// Set the memory size in bytes (≥ 512 MiB).
    pub fn memory_size_bytes(mut self, size: u64) -> Self {
        self.memory_size_bytes = size;
        self
    }

    /// Set the stop timeout.
    pub fn stop_timeout(mut self, timeout: Duration) -> Self {
        self.stop_timeout = timeout;
        self
    }

    /// Set the vsock port for guest control-plane connections.
    ///
    /// The host connects to this port to talk to `vminitd` inside the guest.
    /// Defaults to 1234.
    pub fn vsock_port(mut self, port: u32) -> Self {
        self.vsock_port = port;
        self
    }

    /// Consume the builder and produce a [`GuestConfig`].
    ///
    /// # Panics
    ///
    /// Panics if `kernel_path` was not set.
    pub fn build(self) -> GuestConfig {
        GuestConfig {
            kernel_path: self
                .kernel_path
                .expect("GuestConfigBuilder: kernel_path is required"),
            initrd_path: self.initrd_path,
            cmdline: self.cmdline,
            cpu_count: self.cpu_count,
            memory_size_bytes: self.memory_size_bytes,
            stop_timeout: self.stop_timeout,
            vsock_port: self.vsock_port,
        }
    }
}
