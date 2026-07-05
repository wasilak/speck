use std::path::{Path, PathBuf};
use std::time::Duration;

pub use speck_net::PortMapConfig;
use speck_net::config::NetworkConfig;

/// Configuration for a single VirtioFS volume mount.
#[derive(Debug, Clone)]
pub struct VolumeMountConfig {
    /// Path on the host filesystem.
    pub host_path: PathBuf,
    /// Mount point inside the VM guest (/rootfs{container_path}).
    pub container_path: PathBuf,
    /// Mount read-only.
    pub read_only: bool,
    /// Optional named volume name. `None` for bind mounts.
    pub volume_name: Option<String>,
}

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

    /// Optional network device configuration.
    ///
    /// When set, a `VZVirtioNetworkDeviceConfiguration` with a
    /// `VZFileHandleNetworkDeviceAttachment` is wired into the VM.
    pub network: Option<NetworkConfig>,

    /// Optional vsock port for the DNS resolver proxy inside the guest.
    ///
    /// When set, the host connects to this port after the VM starts
    /// to forward DNS queries to the host's system resolver.
    pub dns_vsock_port: Option<u32>,

    /// Absolute path to the ext4 rootfs disk image.
    ///
    /// The rootfs disk will appear as `/dev/vda` inside the guest.
    /// Must point to a valid ext4 filesystem image; the path must
    /// exist at VM start time.
    pub rootfs_disk_path: Option<PathBuf>,

    /// Absolute path to the ext4 data disk image.
    ///
    /// The data disk will appear as `/dev/vdb` inside the guest and
    /// is mounted at `/var/lib/containerd`.  Must point to a valid
    /// ext4 filesystem image; the path must exist at VM start time.
    pub data_disk_path: Option<PathBuf>,

    /// Vsock port for the containerd gRPC forwarder inside the guest.
    ///
    /// The host connects to this vsock port to forward containerd's
    /// gRPC Unix socket to the host.  Recommended: 9001.
    pub containerd_vsock_port: Option<u32>,

    /// Vsock port for the BuildKit gRPC forwarder inside the guest.
    ///
    /// The host connects to this vsock port to forward BuildKit's
    /// gRPC Unix socket to the host.  Recommended: 9002.
    pub buildkitd_vsock_port: Option<u32>,

    /// Vsock port for the vminitd READY signal.
    ///
    /// vminitd sends a one-byte READY signal on this port once
    /// containerd and BuildKit are fully operational.  Recommended: 9000.
    pub ready_vsock_port: Option<u32>,

    /// Vsock port for the Docker Engine (moby) API forwarder inside the guest.
    ///
    /// vminitd forwards dockerd's Unix socket to this vsock port.
    /// Recommended: 9003.
    pub docker_vsock_port: Option<u32>,

    /// Published TCP port maps (`-p host:container`).
    ///
    /// Each entry spawns a host TcpListener in speck-net after VM start.
    pub port_maps: Vec<PortMapConfig>,

    /// VirtioFS volume mounts.
    ///
    /// Each entry creates a `VZVirtioFileSystemDeviceConfiguration` and
    /// vminitd auto-mounts it at the container path inside the guest.
    pub volume_mounts: Vec<VolumeMountConfig>,

    /// Path to the Speck home directory (for VirtioFS-based Ryuk socket access).
    ///
    /// Defaults to `<data_local_dir>/speck`.
    pub speck_home: PathBuf,

    /// Identity mount roots: host paths shared into the guest at the same
    /// absolute path, enabling Docker bind mount payloads to work without
    /// JSON rewriting.
    ///
    /// For example, adding `/Users` causes the host `/Users` tree to appear
    /// as `/Users` inside the guest rootfs.  Each path generates a stable
    /// VirtioFS tag (`speck-id-<basename>`).  Paths that do not exist on
    /// the host are silently skipped at VM start time.
    pub identity_mounts: Vec<PathBuf>,

    /// Paths to CA certificate PEM files that should be injected into the
    /// guest's trust store at boot time.
    ///
    /// Copied from the host into a VirtioFS share and loaded by vminitd
    /// via `update-ca-certificates`.
    pub ca_certs_paths: Vec<PathBuf>,
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
            network: None,
            dns_vsock_port: None,
            rootfs_disk_path: None,
            data_disk_path: None,
            containerd_vsock_port: None,
            buildkitd_vsock_port: None,
            ready_vsock_port: None,
            docker_vsock_port: None,
            port_maps: Vec::new(),
            volume_mounts: Vec::new(),
            speck_home: default_speck_home(),
            identity_mounts: Vec::new(),
            ca_certs_paths: Vec::new(),
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
        if let Some(ref net) = self.network {
            if net.mtu < 1500 {
                return Err(format!("network MTU must be >= 1500, got {}", net.mtu));
            }
        }
        if let Some(ref rootfs) = self.rootfs_disk_path
            && !rootfs.exists()
        {
            return Err(format!("rootfs disk not found: {}", rootfs.display()));
        }
        if let Some(ref data) = self.data_disk_path
            && !data.exists()
        {
            return Err(format!("data disk not found: {}", data.display()));
        }
        for port_map in &self.port_maps {
            if port_map.host_port <= 1024 {
                return Err(format!(
                    "privileged host port {} is not allowed in port map; use a port > 1024",
                    port_map.host_port
                ));
            }
            if port_map.container_port == 0 {
                return Err("container port must be greater than 0".into());
            }
        }
        for (i, mount) in self.volume_mounts.iter().enumerate() {
            if !mount.host_path.exists() {
                return Err(format!(
                    "volume mount host path does not exist: {}",
                    mount.host_path.display()
                ));
            }
            let container_str = mount.container_path.to_string_lossy();
            if container_str.contains("..") {
                return Err(format!(
                    "container_path must not contain '..': {}",
                    mount.container_path.display()
                ));
            }
            let tag = format!("speck-vol-{i}");
            if !crate::virtiofs::validate_virtiofs_tag(&tag) {
                return Err(format!("invalid VirtioFS tag at index {i}: {tag}"));
            }
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
    network: Option<NetworkConfig>,
    dns_vsock_port: Option<u32>,
    rootfs_disk_path: Option<PathBuf>,
    data_disk_path: Option<PathBuf>,
    containerd_vsock_port: Option<u32>,
    buildkitd_vsock_port: Option<u32>,
    ready_vsock_port: Option<u32>,
    docker_vsock_port: Option<u32>,
    port_maps: Vec<PortMapConfig>,
    volume_mounts: Vec<VolumeMountConfig>,
    speck_home: PathBuf,
    identity_mounts: Vec<PathBuf>,
    ca_certs_paths: Vec<PathBuf>,
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
            network: None,
            dns_vsock_port: None,
            rootfs_disk_path: None,
            data_disk_path: None,
            containerd_vsock_port: None,
            buildkitd_vsock_port: None,
            ready_vsock_port: None,
            docker_vsock_port: None,
            port_maps: Vec::new(),
            volume_mounts: Vec::new(),
            speck_home: default_speck_home(),
            identity_mounts: Vec::new(),
            ca_certs_paths: Vec::new(),
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

    /// Set the network device configuration.
    pub fn network(mut self, network: NetworkConfig) -> Self {
        self.network = Some(network);
        self
    }

    /// Set the vsock port for DNS proxy connections.
    pub fn dns_vsock_port(mut self, port: u32) -> Self {
        self.dns_vsock_port = Some(port);
        self
    }

    /// Set the path to the rootfs ext4 disk image.
    ///
    /// This disk will appear as `/dev/vda` in the guest.
    pub fn rootfs_disk_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.rootfs_disk_path = Some(path.into());
        self
    }

    /// Set the path to the data ext4 disk image.
    ///
    /// This disk will appear as `/dev/vdb` in the guest and
    /// is mounted at `/var/lib/containerd`.
    pub fn data_disk_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_disk_path = Some(path.into());
        self
    }

    /// Set the vsock port for the containerd gRPC forwarder.
    ///
    /// Recommended: 9001.
    pub fn containerd_vsock_port(mut self, port: u32) -> Self {
        self.containerd_vsock_port = Some(port);
        self
    }

    /// Set the vsock port for the BuildKit gRPC forwarder.
    ///
    /// Recommended: 9002.
    pub fn buildkitd_vsock_port(mut self, port: u32) -> Self {
        self.buildkitd_vsock_port = Some(port);
        self
    }

    /// Set the vsock port for the vminitd READY signal.
    ///
    /// Recommended: 9000.
    pub fn ready_vsock_port(mut self, port: u32) -> Self {
        self.ready_vsock_port = Some(port);
        self
    }

    /// Set the vsock port for the Docker Engine (moby) API forwarder.
    ///
    /// Recommended: 9003.
    pub fn docker_vsock_port(mut self, port: u32) -> Self {
        self.docker_vsock_port = Some(port);
        self
    }

    /// Add a published TCP port map (`-p host:container`).
    pub fn add_port_map(mut self, config: PortMapConfig) -> Self {
        self.port_maps.push(config);
        self
    }

    /// Add a VirtioFS volume mount.
    pub fn add_volume_mount(mut self, config: VolumeMountConfig) -> Self {
        self.volume_mounts.push(config);
        self
    }

    /// Set the Speck home directory path.
    pub fn speck_home(mut self, path: impl Into<PathBuf>) -> Self {
        self.speck_home = path.into();
        self
    }

    /// Add an identity mount root.
    ///
    /// The host path will be shared into the guest at the same absolute path.
    /// Paths that do not exist on the host are silently skipped at VM start time.
    pub fn add_identity_mount(mut self, path: impl Into<PathBuf>) -> Self {
        self.identity_mounts.push(path.into());
        self
    }

    /// Set paths to CA certificate PEM files for injection into the guest
    /// trust store.
    ///
    /// These paths must point to PEM-encoded CA certificates on the host.
    /// They are copied into a dedicated VirtioFS share and loaded by vminitd
    /// at boot time via `update-ca-certificates`.
    pub fn ca_certs_paths(mut self, paths: &[impl AsRef<Path>]) -> Self {
        self.ca_certs_paths = paths.iter().map(|p| p.as_ref().to_path_buf()).collect();
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
            network: self.network,
            dns_vsock_port: self.dns_vsock_port,
            rootfs_disk_path: self.rootfs_disk_path,
            data_disk_path: self.data_disk_path,
            containerd_vsock_port: self.containerd_vsock_port,
            buildkitd_vsock_port: self.buildkitd_vsock_port,
            ready_vsock_port: self.ready_vsock_port,
            docker_vsock_port: self.docker_vsock_port,
            port_maps: self.port_maps,
            volume_mounts: self.volume_mounts,
            speck_home: self.speck_home,
            identity_mounts: self.identity_mounts,
            ca_certs_paths: self.ca_certs_paths,
        }
    }
}

fn default_speck_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/share/speck")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guest_config_phase5_fields() {
        let dir = std::env::temp_dir().join("speck-test-phase5");
        std::fs::create_dir_all(&dir).unwrap();
        let kernel = dir.join("Image");
        std::fs::write(&kernel, b"dummy kernel").unwrap();

        let rootfs = dir.join("rootfs.ext4");
        let data = dir.join("data.ext4");

        let config = GuestConfig::builder()
            .kernel_path(&kernel)
            .rootfs_disk_path(&rootfs)
            .data_disk_path(&data)
            .containerd_vsock_port(9001)
            .buildkitd_vsock_port(9002)
            .ready_vsock_port(9000)
            .build();

        // Assert all 5 new fields propagate correctly
        assert_eq!(config.rootfs_disk_path, Some(rootfs.clone()));
        assert_eq!(config.data_disk_path, Some(data.clone()));
        assert_eq!(config.containerd_vsock_port, Some(9001));
        assert_eq!(config.buildkitd_vsock_port, Some(9002));
        assert_eq!(config.ready_vsock_port, Some(9000));

        // Validate with non-existent disk paths returns error about rootfs
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("rootfs"),
            "expected error about rootfs, got: {err}"
        );

        // Clean up
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_guest_config_port_maps() {
        let dir = std::env::temp_dir().join("speck-test-port-maps");
        std::fs::create_dir_all(&dir).unwrap();
        let kernel = dir.join("Image");
        std::fs::write(&kernel, b"dummy kernel").unwrap();

        let config = GuestConfig::builder()
            .kernel_path(&kernel)
            .add_port_map(PortMapConfig {
                host_port: 8080,
                container_port: 80,
            })
            .build();

        assert_eq!(
            config.port_maps,
            vec![PortMapConfig {
                host_port: 8080,
                container_port: 80,
            }]
        );
        assert!(config.validate().is_ok());

        let privileged = GuestConfig::builder()
            .kernel_path(&kernel)
            .add_port_map(PortMapConfig {
                host_port: 1024,
                container_port: 80,
            })
            .build();

        let err = privileged.validate().unwrap_err();
        assert!(err.contains("privileged"), "unexpected error: {err}");
    }

    // ── CA cert paths propagation tests (Phase 12 — RED; method does not exist yet) ──

    #[test]
    fn ca_certs_paths_default_is_empty() {
        let dir = std::env::temp_dir().join("speck-test-ca-paths-default");
        std::fs::create_dir_all(&dir).unwrap();
        let kernel = dir.join("Image");
        std::fs::write(&kernel, b"dummy kernel").unwrap();

        let config = GuestConfig::builder()
            .kernel_path(&kernel)
            .build();

        assert!(
            config.ca_certs_paths.is_empty(),
            "default should have empty ca_certs_paths"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ca_certs_paths_propagates_single_path() {
        let dir = std::env::temp_dir().join("speck-test-ca-paths-single");
        std::fs::create_dir_all(&dir).unwrap();
        let kernel = dir.join("Image");
        std::fs::write(&kernel, b"dummy kernel").unwrap();

        let config = GuestConfig::builder()
            .kernel_path(&kernel)
            .ca_certs_paths(&["/path/to/ca.pem"])
            .build();

        assert_eq!(
            config.ca_certs_paths,
            vec![std::path::PathBuf::from("/path/to/ca.pem")],
            "single ca_certs_path should propagate"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ca_certs_paths_propagates_multiple_paths() {
        let dir = std::env::temp_dir().join("speck-test-ca-paths-multi");
        std::fs::create_dir_all(&dir).unwrap();
        let kernel = dir.join("Image");
        std::fs::write(&kernel, b"dummy kernel").unwrap();

        let paths = vec!["/etc/certs/root.pem", "/etc/certs/intermediate.pem"];
        let config = GuestConfig::builder()
            .kernel_path(&kernel)
            .ca_certs_paths(&paths)
            .build();

        assert_eq!(
            config.ca_certs_paths,
            vec![
                std::path::PathBuf::from("/etc/certs/root.pem"),
                std::path::PathBuf::from("/etc/certs/intermediate.pem"),
            ],
            "multiple ca_certs_paths should propagate in order"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_identity_mounts_propagate() {
        let dir = std::env::temp_dir().join("speck-test-identity-mounts");
        std::fs::create_dir_all(&dir).unwrap();
        let kernel = dir.join("Image");
        std::fs::write(&kernel, b"dummy kernel").unwrap();

        let config = GuestConfig::builder()
            .kernel_path(&kernel)
            .add_identity_mount("/Users")
            .add_identity_mount("/Volumes")
            .build();

        assert_eq!(config.identity_mounts.len(), 2);
        assert_eq!(config.identity_mounts[0], PathBuf::from("/Users"));
        assert_eq!(config.identity_mounts[1], PathBuf::from("/Volumes"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
