use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

use anstream::{print, println};

use crate::docker_client::DockerClient;
use crate::output::{self, Output};
use crate::profile::{self, Profile};
use crate::theme;

/// What `spk status`, `spk list` and `spk version` report (colima parity, UX-04).
#[derive(Debug, Default, Serialize)]
pub struct Info {
    pub profile: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub arch: &'static str,
    pub runtime: &'static str,
    pub socket: String,
    pub docker_context: Option<String>,
    pub network: &'static str,
    pub cpus: Option<u64>,
    pub memory_mb: Option<u64>,
    pub disk_gb: Option<u64>,
    pub disk_used_bytes: Option<u64>,
    pub docker_version: Option<String>,
    pub containers: Option<u64>,
    pub containers_running: Option<u64>,
    pub images: Option<u64>,
    pub versions: Versions,
}

#[derive(Debug, Default, Serialize)]
pub struct Versions {
    pub spk: &'static str,
    pub initrd: Option<String>,
    pub rootfs: Option<String>,
    pub kernel: Option<String>,
}

/// Probe the daemon's control socket. The daemon's control server reads first
/// (PREPARE_RESTART protocol), so write PING (any unknown command elicits
/// PONG), then drain the reply under a 2s timeout. Liveness is defined by
/// connect success.
async fn daemon_running(speck_home: &Path) -> bool {
    let sock_path = speck_home.join("run/control.sock");
    match timeout(Duration::from_secs(2), UnixStream::connect(&sock_path)).await {
        Ok(Ok(mut stream)) => {
            let _ = stream.write_all(b"PING\n").await;
            let mut buf = [0u8; 8];
            let _ = timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
            true
        }
        _ => false,
    }
}

pub async fn gather(profile: &Profile) -> Info {
    let speck_home = profile.home.as_path();
    let assets = profile.assets.as_path();
    let sock = speck_home.join("speck.sock");
    let mut info = Info {
        profile: profile.name.clone(),
        running: daemon_running(speck_home).await,
        arch: "aarch64",
        runtime: "docker",
        socket: format!("unix://{}", sock.display()),
        network: "host-inheriting user-space netstack (follows VPN/WARP routes and DNS)",
        versions: Versions {
            spk: env!("CARGO_PKG_VERSION"),
            initrd: asset_version(
                &assets.join("initrd/initrd.version"),
                &assets.join("initrd/initrd.cpio.gz"),
            ),
            rootfs: asset_version(
                &speck_home.join("rootfs.version"),
                &speck_home.join("rootfs.img"),
            ),
            kernel: std::fs::read_link(assets.join("kernel/vmlinux"))
                .ok()
                .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
                .map(|n| n.trim_start_matches("vmlinux-").to_owned()),
        },
        ..Info::default()
    };

    if let Ok(Some(vm)) = crate::commands::up::read_vm_resource_snapshot(speck_home) {
        info.cpus = Some(vm.cpus);
        info.memory_mb = Some(vm.memory_mb);
        info.disk_gb = Some(vm.disk_gb);
    }
    if let Ok(meta) = std::fs::metadata(speck_home.join("data.img")) {
        use std::os::unix::fs::MetadataExt as _;
        info.disk_used_bytes = Some(meta.blocks() * 512);
    }
    if !info.running {
        return info;
    }

    if let Ok(pid_str) = std::fs::read_to_string(speck_home.join("run/speck.pid"))
        && let Ok(pid) = pid_str.trim().parse::<u32>()
    {
        info.pid = Some(pid);
        let mut sys = sysinfo::System::new();
        let spid = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[spid]), true);
        info.uptime_secs = sys.process(spid).map(|p| p.run_time());
    }
    info.docker_context = crate::docker_context::current();

    let client = DockerClient::new(&sock);
    let docker_timeout = Duration::from_secs(3);
    if let Ok(Ok(v)) = timeout(docker_timeout, client.get("/version")).await {
        info.docker_version = v.get("Version").and_then(|x| x.as_str()).map(str::to_owned);
    }
    if let Ok(Ok(v)) = timeout(docker_timeout, client.get("/info")).await {
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64());
        info.containers = n("Containers");
        info.containers_running = n("ContainersRunning");
        info.images = n("Images");
    }
    info
}

/// Version of a guest asset: its version file, "unversioned" for a local
/// build without one, `None` when the asset is not installed.
fn asset_version(version_file: &Path, asset: &Path) -> Option<String> {
    read_trimmed(version_file).or_else(|| asset.exists().then(|| "unversioned".to_owned()))
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// `spk status [-e] [-j]`. Returns the exit code: 1 when Speck is not
/// running, like `colima status`.
pub async fn run_status(extended: bool, format: Output) -> anyhow::Result<i32> {
    let info = gather(&profile::current()).await;
    if format.is_plain() {
        print!("{}", render_status(&info, extended));
    } else {
        output::emit(format, &info)?;
    }
    Ok(if info.running { 0 } else { 1 })
}

pub fn render_status(info: &Info, extended: bool) -> String {
    let mut out = String::new();
    let name = if info.profile.is_empty() || info.profile == profile::DEFAULT {
        "Speck".to_owned()
    } else {
        format!("Speck profile \"{}\"", info.profile)
    };
    if !info.running {
        out.push_str(&format!("{name} is {}\n", theme::format_status("stopped")));
        return out;
    }
    let mut head = format!("{name} is {}", theme::format_status("running"));
    match (info.pid, info.uptime_secs) {
        (Some(pid), Some(up)) => {
            head.push_str(&format!(" (PID {pid}, up {})", format_duration(up)))
        }
        (Some(pid), None) => head.push_str(&format!(" (PID {pid})")),
        _ => {}
    }
    out.push_str(&head);
    out.push('\n');
    let unknown = || "unknown".to_owned();
    let mut line = |k: &str, v: String| out.push_str(&format!("  {k:<11} {v}\n"));
    line("arch:", info.arch.to_owned());
    line(
        "runtime:",
        match &info.docker_version {
            Some(v) => format!("{} {v}", info.runtime),
            None => format!("{} (not responding)", info.runtime),
        },
    );
    line("socket:", info.socket.clone());
    line(
        "context:",
        info.docker_context.clone().unwrap_or_else(unknown),
    );
    line("network:", info.network.to_owned());
    if extended {
        line("cpus:", info.cpus.map_or_else(unknown, |c| c.to_string()));
        line(
            "memory:",
            info.memory_mb.map_or_else(unknown, |m| format!("{m} MiB")),
        );
        line(
            "disk:",
            match (info.disk_gb, info.disk_used_bytes) {
                (Some(g), Some(u)) => format!("{g} GiB ({} used)", format_bytes(u)),
                (Some(g), None) => format!("{g} GiB"),
                _ => unknown(),
            },
        );
        line(
            "containers:",
            match (info.containers_running, info.containers) {
                (Some(r), Some(t)) => format!("{r} running, {t} total"),
                _ => unknown(),
            },
        );
        line(
            "images:",
            info.images.map_or_else(unknown, |i| i.to_string()),
        );
        line("versions:", render_versions(&info.versions));
    }
    out
}

fn render_versions(v: &Versions) -> String {
    let opt = |o: &Option<String>| o.clone().unwrap_or_else(|| "unknown".to_owned());
    format!(
        "spk {}, initrd {}, rootfs {}, kernel {}",
        v.spk,
        opt(&v.initrd),
        opt(&v.rootfs),
        opt(&v.kernel)
    )
}

/// Profiles `spk list` shows: every profile, or only the raw `SPECK_HOME`
/// instance.
pub fn listed_profiles() -> Vec<Profile> {
    let current = profile::current();
    if current.raw_home {
        return vec![current];
    }
    let user_home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    profile::all(Path::new(&user_home))
}

/// `spk list [-j]`: one row per profile, colima list columns.
/// One `spk list -o json|yaml` entry: the status of a profile and whether it
/// is the current one.
#[derive(Serialize)]
struct ListEntry<'a> {
    #[serde(flatten)]
    info: &'a Info,
    current: bool,
}

pub async fn run_list(format: Output) -> anyhow::Result<()> {
    let mut infos = Vec::new();
    for profile in listed_profiles() {
        infos.push(gather(&profile).await);
    }
    let current = profile::current().name;
    if format.is_plain() {
        print!("{}", render_list(&infos, &current));
    } else {
        let entries: Vec<ListEntry> = infos
            .iter()
            .map(|info| ListEntry {
                info,
                current: info.profile == current,
            })
            .collect();
        output::emit(format, &entries)?;
    }
    Ok(())
}

/// With more than one profile, `current` is marked with `*`.
pub fn render_list(infos: &[Info], current: &str) -> String {
    let gib = |mb: u64| format!("{}GiB", mb / 1024);
    let header = [
        "PROFILE", "STATUS", "ARCH", "CPUS", "MEMORY", "DISK", "RUNTIME",
    ];
    let rows: Vec<[String; 7]> = infos
        .iter()
        .map(|info| {
            [
                {
                    let name = if info.profile.is_empty() {
                        profile::DEFAULT
                    } else {
                        info.profile.as_str()
                    };
                    if infos.len() > 1 && name == current {
                        format!("{name} *")
                    } else {
                        name.to_owned()
                    }
                },
                if info.running { "Running" } else { "Stopped" }.to_owned(),
                info.arch.to_owned(),
                info.cpus.map_or("-".into(), |c| c.to_string()),
                info.memory_mb.map_or("-".into(), gib),
                info.disk_gb.map_or("-".into(), |d| format!("{d}GiB")),
                info.runtime.to_owned(),
            ]
        })
        .collect();
    let widths: Vec<usize> = (0..header.len())
        .map(|i| {
            rows.iter()
                .map(|r| r[i].len())
                .chain([header[i].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let fmt = |cells: Vec<&str>| {
        let mut l = cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect::<Vec<_>>()
            .join("    ");
        l.truncate(l.trim_end().len());
        l + "\n"
    };
    let mut out = fmt(header.to_vec());
    for row in &rows {
        out += &fmt(row.iter().map(String::as_str).collect());
    }
    out
}

/// `spk version`: Speck, runtime and guest versions.
/// `spk version -o json|yaml` document.
#[derive(Serialize)]
struct VersionDoc<'a> {
    spk: &'a str,
    docker: Option<&'a str>,
    initrd: Option<&'a str>,
    rootfs: Option<&'a str>,
    kernel: Option<&'a str>,
}

pub async fn run_version(format: Output) -> anyhow::Result<()> {
    let info = gather(&profile::current()).await;
    if !format.is_plain() {
        return output::emit(
            format,
            &VersionDoc {
                spk: info.versions.spk,
                docker: info.docker_version.as_deref(),
                initrd: info.versions.initrd.as_deref(),
                rootfs: info.versions.rootfs.as_deref(),
                kernel: info.versions.kernel.as_deref(),
            },
        );
    }
    println!("spk version {}", info.versions.spk);
    match &info.docker_version {
        Some(v) => println!("runtime: docker {v}"),
        None => println!("runtime: docker (Speck not running)"),
    }
    let opt = |o: &Option<String>| o.clone().unwrap_or_else(|| "not installed".to_owned());
    println!("initrd: {}", opt(&info.versions.initrd));
    println!("rootfs: {}", opt(&info.versions.rootfs));
    println!("kernel: {}", opt(&info.versions.kernel));
    Ok(())
}

fn format_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

fn format_bytes(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gib >= 1.0 {
        format!("{gib:.1} GiB")
    } else {
        format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_info() -> Info {
        Info {
            profile: "default".into(),
            running: true,
            pid: Some(42),
            uptime_secs: Some(3 * 3600 + 7 * 60),
            arch: "aarch64",
            runtime: "docker",
            socket: "unix:///Users/x/.speck/speck.sock".into(),
            docker_context: Some("speck".into()),
            network: "host",
            cpus: Some(2),
            memory_mb: Some(4096),
            disk_gb: Some(20),
            disk_used_bytes: Some(3 * 1024 * 1024 * 1024 + 512 * 1024 * 1024),
            docker_version: Some("26.1.5".into()),
            containers: Some(5),
            containers_running: Some(2),
            images: Some(20),
            versions: Versions {
                spk: "1.4.1",
                initrd: Some("1.4.0".into()),
                rootfs: Some("1.3.0".into()),
                kernel: Some("6.18.35-197".into()),
            },
        }
    }

    #[test]
    fn status_shows_runtime_socket_and_context() {
        let out = render_status(&running_info(), false);
        assert!(out.contains("PID 42, up 3h 7m"));
        assert!(out.contains("docker 26.1.5"));
        assert!(out.contains("unix:///Users/x/.speck/speck.sock"));
        assert!(out.contains("context:    speck"));
        assert!(!out.contains("images:"));
    }

    #[test]
    fn extended_status_adds_resources_and_versions() {
        let out = render_status(&running_info(), true);
        assert!(out.contains("20 GiB (3.5 GiB used)"));
        assert!(out.contains("2 running, 5 total"));
        assert!(out.contains("images:     20"));
        assert!(out.contains("spk 1.4.1, initrd 1.4.0, rootfs 1.3.0, kernel 6.18.35-197"));
    }

    #[test]
    fn stopped_status_is_one_line() {
        let out = render_status(&Info::default(), true);
        assert_eq!(out.lines().count(), 1);
        assert!(out.contains("stopped"));
    }

    #[test]
    fn list_has_colima_columns() {
        let out = render_list(&[running_info()], "default");
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("PROFILE"));
        assert!(lines[0].contains("RUNTIME"));
        assert!(lines[1].contains("Running"));
        assert!(lines[1].contains("4GiB"));
        assert!(lines[1].contains("20GiB"));
    }

    #[test]
    fn list_has_one_row_per_profile() {
        let work = Info {
            profile: "work-long-name".into(),
            arch: "aarch64",
            runtime: "docker",
            ..Info::default()
        };
        let out = render_list(&[running_info(), work], "work-long-name");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        let words = |l: &str| l.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        assert_eq!(words(lines[1]), "default Running");
        assert_eq!(words(lines[2]), "work-long-name *");
        // Columns line up across rows.
        assert_eq!(lines[1].find("Running"), lines[2].find("Stopped"));
        assert!(!render_list(&[running_info()], "default").contains('*'));
    }

    #[test]
    fn named_profiles_are_named_in_status() {
        let mut info = running_info();
        info.profile = "work".into();
        assert!(render_status(&info, false).starts_with("Speck profile \"work\" is"));
        info.running = false;
        assert!(render_status(&info, false).contains("Speck profile \"work\" is"));
        assert!(render_status(&running_info(), false).starts_with("Speck is"));
    }

    #[test]
    fn list_entries_mark_the_current_profile() {
        let info = running_info();
        let v: serde_json::Value = serde_json::from_str(
            &output::render(
                Output::Json,
                &[ListEntry {
                    info: &info,
                    current: true,
                }],
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(v[0]["profile"], "default");
        assert_eq!(v[0]["current"], true);
        assert_eq!(v[0]["running"], true);
    }

    #[test]
    fn json_status_is_machine_readable() {
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&running_info()).unwrap()).unwrap();
        assert_eq!(v["running"], true);
        assert_eq!(v["versions"]["initrd"], "1.4.0");
    }

    #[test]
    fn unversioned_local_assets_are_not_reported_missing() {
        let dir = std::env::temp_dir().join(format!("speck-status-ver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (ver, img) = (dir.join("rootfs.version"), dir.join("rootfs.img"));
        assert_eq!(asset_version(&ver, &img), None);
        std::fs::write(&img, "x").unwrap();
        assert_eq!(asset_version(&ver, &img).as_deref(), Some("unversioned"));
        std::fs::write(&ver, "1.3.0\n").unwrap();
        assert_eq!(asset_version(&ver, &img).as_deref(), Some("1.3.0"));
    }

    #[test]
    fn durations_and_sizes_are_human_readable() {
        assert_eq!(format_duration(59), "0m");
        assert_eq!(format_duration(3 * 86_400 + 5 * 3600), "3d 5h");
        assert_eq!(format_bytes(300 * 1024 * 1024), "300 MiB");
    }

    const STATUS_SOURCE: &str = include_str!("status.rs");

    /// Slice production code only — everything before `#[cfg(test)]` — so tests do not
    /// trivially pass because assertion strings themselves contain the searched tokens.
    fn production_code() -> &'static str {
        let end = STATUS_SOURCE
            .find("#[cfg(test)]")
            .unwrap_or(STATUS_SOURCE.len());
        &STATUS_SOURCE[..end]
    }

    #[test]
    fn status_reads_pid_from_run_directory() {
        assert!(
            production_code().contains("run/speck.pid"),
            "status must read PID from speck_home/run/speck.pid (consistent with up.rs write location)"
        );
    }

    #[test]
    fn status_probe_writes_ping_before_read() {
        let src = production_code();
        let ping_pos = src
            .find("write_all(b\"PING")
            .expect("run_status probe must write PING before reading (daemon reads first)");
        let read_pos = src
            .find("stream.read")
            .expect("run_status probe must read the PONG reply");
        assert!(
            ping_pos < read_pos,
            "PING write must precede the read — the daemon's control server reads first"
        );
        assert!(
            src.contains("timeout(Duration::from_secs(2), stream.read"),
            "run_status probe read must be bounded by a 2-second timeout"
        );
    }
}
