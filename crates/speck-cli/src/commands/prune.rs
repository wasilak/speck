use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use anstream::println;

/// Cached files `spk start` does not use: old initrd/kernel copies, scratch
/// downloads and temp files. The active initrd, its version file, the Kata
/// initrd (its absence would trigger a kernel re-download) and the kernel the
/// `vmlinux` symlink points to are kept, as are VM disks and config.
pub fn prune_candidates(speck_home: &Path) -> Vec<PathBuf> {
    let keep_initrd = [
        "initrd.cpio.gz",
        "initrd.version",
        crate::commands::up::KATA_INITRD_FILE,
    ];
    let kernel_target = std::fs::read_link(speck_home.join("kernel/vmlinux"))
        .ok()
        .and_then(|t| t.file_name().map(|n| n.to_owned()));

    let mut out = Vec::new();
    let mut scan = |dir: &str, keep: &dyn Fn(&str) -> bool| {
        for entry in std::fs::read_dir(speck_home.join(dir))
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !keep(&name) {
                out.push(entry.path());
            }
        }
    };
    scan("initrd", &|n| keep_initrd.contains(&n));
    scan("kernel", &|n| {
        n == "vmlinux" || kernel_target.as_deref().is_some_and(|t| t == n)
    });
    scan("tmp", &|_| false);
    scan(".", &|n| !n.ends_with(".tmp"));
    out.sort();
    out
}

fn size_of(path: &Path) -> u64 {
    if path.is_dir() {
        walkdir::WalkDir::new(path)
            .into_iter()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
    } else {
        std::fs::symlink_metadata(path)
            .map(|m| m.len())
            .unwrap_or(0)
    }
}

/// `spk prune [--force]`: delete cached downloads Speck no longer uses
/// (colima prune).
pub fn run_prune(force: bool, speck_home: &Path) -> anyhow::Result<()> {
    let candidates = prune_candidates(speck_home);
    if candidates.is_empty() {
        println!("Nothing to prune.");
        return Ok(());
    }
    let total: u64 = candidates.iter().map(|p| size_of(p)).sum();
    for path in &candidates {
        println!("  {}", path.display());
    }
    println!(
        "{} file(s), {:.1} MiB",
        candidates.len(),
        total as f64 / 1048576.0
    );

    if !force {
        anyhow::ensure!(
            std::io::stdin().is_terminal(),
            "re-run with --force to delete these files"
        );
        print!("Delete them? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }
    for path in &candidates {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        removed.map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", path.display()))?;
    }
    println!("Pruned {:.1} MiB.", total as f64 / 1048576.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unused_cache_files_are_pruned() {
        let home = std::env::temp_dir().join(format!("speck-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for d in ["initrd", "kernel", "tmp/x"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        for f in [
            "initrd/initrd.cpio.gz",
            "initrd/initrd.version",
            "initrd/kata-alpine-3.22.initrd",
            "initrd/initrd.cpio.gz.bak",
            "initrd/initrd.cpio.gz.original",
            "kernel/vmlinux-6.18.35-197",
            "kernel/vmlinux-6.1.0-old",
            "tmp/x/partial",
            "rootfs.img.gz.tmp",
            "data.img",
            "rootfs.img",
            "config.yaml",
        ] {
            std::fs::write(home.join(f), "x").unwrap();
        }
        std::os::unix::fs::symlink("vmlinux-6.18.35-197", home.join("kernel/vmlinux")).unwrap();

        let names: Vec<String> = prune_candidates(&home)
            .iter()
            .map(|p| p.strip_prefix(&home).unwrap().display().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "initrd/initrd.cpio.gz.bak",
                "initrd/initrd.cpio.gz.original",
                "kernel/vmlinux-6.1.0-old",
                "rootfs.img.gz.tmp",
                "tmp/x",
            ]
        );
    }
}
