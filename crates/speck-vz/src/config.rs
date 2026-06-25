/// Guest configuration — full implementation in Plan 02-02.
///
/// Defines the kernel image, CPU count, and memory size for the
/// micro-VM. This is a minimal stub that lets the rest of the
/// binding layer compile; the real builder pattern + validation
/// will be added when VZVirtualMachineConfiguration is wired up.
#[derive(Debug, Clone)]
pub struct GuestConfig {
    /// Absolute path to the guest Linux kernel image.
    pub kernel_path: std::path::PathBuf,

    /// Number of virtual CPUs (must be ≥ 1).
    pub cpu_count: u64,

    /// Memory size in bytes (must be ≥ 512 MiB).
    pub memory_size_bytes: u64,
}
