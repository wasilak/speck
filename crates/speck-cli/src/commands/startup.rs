//! Startup progress, so `spk status` can say "starting" instead of "stopped"
//! while the daemon boots. The daemon writes `run/startup` when it begins,
//! appends each finished stage, and removes the file once it is running or
//! when it exits.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The stages in order: id written to the file, and the label status shows.
pub const STAGES: &[(&str, &str)] = &[
    ("assets", "VM assets"),
    ("vm", "VM boot"),
    ("guest", "guest init"),
    ("network", "network"),
    ("docker", "Docker API"),
];

/// Written by the daemon. Dropping it removes the file.
pub struct Progress {
    path: PathBuf,
}

impl Progress {
    pub fn begin(speck_home: &Path) -> std::io::Result<Self> {
        let path = speck_home.join("run/startup");
        std::fs::create_dir_all(speck_home.join("run"))?;
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        std::fs::write(&path, format!("{} {started}\n", std::process::id()))?;
        Ok(Self { path })
    }

    pub fn done(&self, stage: &str) {
        mark_done(&self.path, stage);
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Append a finished stage. Usable from a blocking task through the path.
pub fn mark_done(path: &Path, stage: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
        let _ = writeln!(f, "{stage}");
    }
}

#[derive(Debug, Serialize)]
pub struct Startup {
    pub pid: u32,
    pub elapsed_secs: u64,
    pub stages: Vec<Stage>,
}

#[derive(Debug, Serialize)]
pub struct Stage {
    pub name: &'static str,
    pub label: &'static str,
    pub done: bool,
}

/// The startup in progress, or `None` when no live daemon is starting.
pub fn read(speck_home: &Path) -> Option<Startup> {
    let text = std::fs::read_to_string(speck_home.join("run/startup")).ok()?;
    let startup = parse(&text, now_secs())?;
    pid_alive(startup.pid).then_some(startup)
}

/// After `spk start` launched the daemon: wait up to 3 s until it records
/// its startup, so an immediate `spk status` does not report "stopped".
pub fn wait_until_recorded(speck_home: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if speck_home.join("run/startup").exists() || speck_home.join("run/control.sock").exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn parse(text: &str, now: u64) -> Option<Startup> {
    let mut lines = text.lines();
    let mut head = lines.next()?.split_whitespace();
    let pid = head.next()?.parse().ok()?;
    let started: u64 = head.next()?.parse().ok()?;
    let finished: Vec<&str> = lines.collect();
    Some(Startup {
        pid,
        elapsed_secs: now.saturating_sub(started),
        stages: STAGES
            .iter()
            .map(|&(name, label)| Stage {
                name,
                label,
                done: finished.contains(&name),
            })
            .collect(),
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn pid_alive(pid: u32) -> bool {
    pid != 0
        && std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_marks_finished_stages_in_order() {
        let s = parse("42 100\nassets\nvm\n", 107).unwrap();
        assert_eq!(s.pid, 42);
        assert_eq!(s.elapsed_secs, 7);
        let done: Vec<bool> = s.stages.iter().map(|st| st.done).collect();
        assert_eq!(done, [true, true, false, false, false]);
    }

    #[test]
    fn parse_rejects_a_broken_header() {
        assert!(parse("", 0).is_none());
        assert!(parse("abc 1\n", 0).is_none());
    }

    #[test]
    fn progress_file_lives_until_dropped() {
        let home = std::env::temp_dir().join(format!("speck-startup-{}", std::process::id()));
        let progress = Progress::begin(&home).unwrap();
        progress.done("assets");
        let startup = read(&home).expect("own process is alive");
        assert_eq!(startup.pid, std::process::id());
        assert!(startup.stages[0].done && !startup.stages[1].done);
        drop(progress);
        assert!(read(&home).is_none());
        let _ = std::fs::remove_dir_all(&home);
    }
}
