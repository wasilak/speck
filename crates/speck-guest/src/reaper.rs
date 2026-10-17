//! Reaping of orphaned processes by PID 1.
//!
//! containerd shims daemonize, so when one exits its zombie is re-parented to
//! vminitd. Without reaping, every finished container leaves a zombie behind
//! until the guest runs out of PIDs. vminitd's own children (dockerd, helper
//! commands) are waited for by the threads that spawned them; a blanket
//! `waitpid(-1)` would steal their exit status. The reaper therefore only
//! reaps zombies that were already zombies one scan earlier: a child someone
//! waits for is gone well before the next scan.

use std::collections::HashSet;

/// How often PID 1 scans for zombie children.
pub const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Zombies to reap now: the ones seen in both the previous and this scan.
pub fn orphans_to_reap(previous: &HashSet<i32>, current: &HashSet<i32>) -> Vec<i32> {
    let mut pids: Vec<i32> = previous.intersection(current).copied().collect();
    pids.sort_unstable();
    pids
}

/// Whether a `/proc/<pid>/stat` line describes a zombie child of PID 1.
pub fn is_zombie_child_of_init(stat: &str) -> bool {
    // Format: "pid (comm) state ppid ...". comm may contain spaces and
    // parentheses, so parse from the last ')'.
    let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
        return false;
    };
    let mut fields = rest.split_whitespace();
    fields.next() == Some("Z") && fields.next() == Some("1")
}

/// Scan and reap forever. Never returns; PID 1 runs this on its main thread.
#[cfg(target_os = "linux")]
pub fn run() -> ! {
    let mut previous = HashSet::new();
    loop {
        std::thread::sleep(SCAN_INTERVAL);
        let current = zombie_children_of_init();
        for pid in orphans_to_reap(&previous, &current) {
            let mut status = 0;
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        }
        previous = current;
    }
}

#[cfg(target_os = "linux")]
fn zombie_children_of_init() -> HashSet<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return HashSet::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|stat| is_zombie_child_of_init(&stat))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaps_only_zombies_seen_twice() {
        let previous: HashSet<i32> = [10, 11, 12].into();
        let current: HashSet<i32> = [11, 12, 13].into();
        assert_eq!(orphans_to_reap(&previous, &current), vec![11, 12]);
        assert!(orphans_to_reap(&HashSet::new(), &current).is_empty());
    }

    #[test]
    fn parses_zombie_children_of_init() {
        assert!(is_zombie_child_of_init(
            "1234 (containerd-shim) Z 1 1234 1234 0 -1 4194372"
        ));
        assert!(is_zombie_child_of_init("77 (odd ) name) Z 1 77 77"));
        assert!(!is_zombie_child_of_init("1234 (containerd-shim) S 1 1234"));
        assert!(!is_zombie_child_of_init("1234 (sh) Z 512 1234"));
        assert!(!is_zombie_child_of_init("garbage"));
    }
}
