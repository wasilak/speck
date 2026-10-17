//! Memory reserved for the guest's own runtime.
//!
//! Containers and build steps run in the `docker` cgroup (cgroupfs driver,
//! cgroup v2). Capping that cgroup below the guest RAM keeps the kernel,
//! vminitd, dockerd, containerd and the per-container shims out of reach of
//! containers: when containers fill their share, the kernel OOM-kills inside
//! the cgroup and the Docker API keeps answering.

/// cgroup that dockerd puts containers (and BuildKit steps) under.
pub const CONTAINERS_CGROUP: &str = "/sys/fs/cgroup/docker";

const MIB: u64 = 1024 * 1024;
const MIN_RESERVE: u64 = 512 * MIB;
const MAX_RESERVE: u64 = 1024 * MIB;

/// `memory.max` for the containers cgroup: total RAM minus one eighth of it,
/// at least 512 MiB and at most 1 GiB.
pub fn containers_memory_max(total_bytes: u64) -> u64 {
    let reserve = (total_bytes / 8).clamp(MIN_RESERVE, MAX_RESERVE);
    total_bytes.saturating_sub(reserve)
}

/// Total RAM in bytes from a `/proc/meminfo` text.
pub fn mem_total_bytes(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// Create the containers cgroup with its memory cap. Runs before dockerd.
#[cfg(target_os = "linux")]
pub fn reserve_for_runtime() -> std::io::Result<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")?;
    let total = mem_total_bytes(&meminfo)
        .ok_or_else(|| std::io::Error::other("no MemTotal in /proc/meminfo"))?;
    let max = containers_memory_max(total);
    std::fs::write(
        "/sys/fs/cgroup/cgroup.subtree_control",
        "+cpuset +cpu +io +memory +pids",
    )?;
    std::fs::create_dir_all(CONTAINERS_CGROUP)?;
    std::fs::write(format!("{CONTAINERS_CGROUP}/memory.max"), max.to_string())?;
    Ok(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_is_an_eighth_within_bounds() {
        let gib = 1024 * MIB;
        assert_eq!(containers_memory_max(2 * gib), 2 * gib - 512 * MIB);
        assert_eq!(containers_memory_max(6 * gib), 6 * gib - 768 * MIB);
        assert_eq!(containers_memory_max(16 * gib), 15 * gib);
        assert_eq!(containers_memory_max(256 * MIB), 0);
    }

    #[test]
    fn reads_mem_total() {
        let meminfo = "MemTotal:        2037924 kB\nMemFree:          123 kB\n";
        assert_eq!(mem_total_bytes(meminfo), Some(2_037_924 * 1024));
        assert_eq!(mem_total_bytes("MemFree: 1 kB\n"), None);
    }
}
