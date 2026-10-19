use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use anstream::println;

use crate::config;
use crate::docker_client::DockerClient;
use crate::output::{self, Output};
use crate::theme::{GREEN, RED, RESET, YELLOW};
use crate::{DoctorArgs, DoctorSubcommand};

/// Outcome of a single health check performed by `spk doctor`.
#[derive(Debug, PartialEq, Eq)]
pub enum CheckResult {
    Pass,
    PassWithDetail { detail: String },
    Fail { hint: String },
    Warn { detail: String },
    Skip { reason: String },
}

/// One check in the `spk doctor -o json|yaml` document.
#[derive(Debug, serde::Serialize)]
pub struct CheckDoc {
    pub name: String,
    /// `pass`, `fail`, `warn` or `info`.
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl CheckDoc {
    pub fn new(name: &str, result: &CheckResult) -> Self {
        let (status, detail) = match result {
            CheckResult::Pass => ("pass", None),
            CheckResult::PassWithDetail { detail } => ("pass", Some(detail)),
            CheckResult::Fail { hint } => ("fail", Some(hint)),
            CheckResult::Warn { detail } => ("warn", Some(detail)),
            CheckResult::Skip { reason } => ("info", Some(reason)),
        };
        Self {
            name: name.to_owned(),
            status,
            detail: detail.cloned(),
        }
    }
}

#[derive(serde::Serialize)]
struct DoctorDoc {
    speck_home: String,
    ok: bool,
    checks: Vec<CheckDoc>,
}

/// Print one check result line to stdout using the Speck colour scheme.
pub fn print_check(label: &str, result: &CheckResult) {
    match result {
        CheckResult::Pass => println!("  {GREEN}PASS{RESET}  {label}"),
        CheckResult::PassWithDetail { detail } => {
            println!("  {GREEN}PASS{RESET}  {label}\n         {detail}")
        }
        CheckResult::Fail { hint } => {
            println!("  {RED}FAIL{RESET}  {label}\n         hint: {hint}")
        }
        CheckResult::Warn { detail } => {
            println!("  {YELLOW}WARN{RESET}  {label}\n         {detail}")
        }
        CheckResult::Skip { reason } => println!("  {YELLOW}INFO{RESET}  {label} ({reason})"),
    }
}

/// Verify the binary carries the `com.apple.security.virtualization` entitlement.
///
/// Combines stdout and stderr from `codesign -d --entitlements -` because macOS
/// routes the plist output to different streams depending on the OS version.
pub fn check_codesign() -> CheckResult {
    let binary_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => {
            return CheckResult::Warn {
                detail: "could not determine binary path".into(),
            };
        }
    };

    match std::process::Command::new("codesign")
        .args(["-d", "--entitlements", "-", &binary_path.to_string_lossy()])
        .output()
    {
        Ok(output) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if combined.contains("com.apple.security.virtualization") {
                CheckResult::Pass
            } else {
                CheckResult::Fail {
                    hint:
                        "Binary missing virtualization entitlement — run `cargo xtask codesign-dev`"
                            .into(),
                }
            }
        }
        Err(_) => CheckResult::Warn {
            detail: "codesign not available (expected on macOS)".into(),
        },
    }
}

/// Check that DOCKER_HOST points at Speck's socket.
///
/// Returns Warn (not Fail) so that running `spk doctor` without `spk up` does not
/// block on a red exit code — the user just needs to run `eval $(spk env)`.
pub fn check_docker_host(speck_home: &Path) -> CheckResult {
    let expected = format!("unix://{}", speck_home.join("speck.sock").display());
    match std::env::var("DOCKER_HOST") {
        Ok(val) if val == expected => CheckResult::Pass,
        Ok(val) => CheckResult::Warn {
            detail: format!(
                "DOCKER_HOST={val} (expected {expected}) — run `eval $(spk env)` or check for another active runtime"
            ),
        },
        Err(_) => CheckResult::Warn {
            detail: "DOCKER_HOST not set — run `eval $(spk env)` or `spk init`".into(),
        },
    }
}

/// Verify every CA cert listed in `config.yaml` has been staged into `{speck_home}/ca-certs/`.
pub fn check_cert_injection(speck_home: &Path) -> CheckResult {
    let (app_config, _) = match config::load_config_file(speck_home) {
        Ok(r) => r,
        Err(e) => {
            return CheckResult::Warn {
                detail: format!("cannot read config: {e}"),
            };
        }
    };

    if app_config.ca.extra_certs.is_empty() {
        return CheckResult::PassWithDetail {
            detail: format!(
                "no CA certs configured — add paths to ca.extra_certs in {}/config.yaml",
                speck_home.display()
            ),
        };
    }

    for path_str in &app_config.ca.extra_certs {
        let content = match std::fs::read(path_str) {
            Ok(c) => c,
            Err(_) => {
                return CheckResult::Fail {
                    hint: format!("configured cert source not found: {path_str}"),
                };
            }
        };
        let hash_hex = format!("{:x}", Sha256::digest(&content));
        let staged = speck_home.join("ca-certs").join(format!("{hash_hex}.pem"));
        if !staged.exists() {
            return CheckResult::Fail {
                hint: format!("cert {path_str} not staged — run `spk up` to reinject"),
            };
        }
    }

    CheckResult::Pass
}

/// Report VM CPU / memory / disk from the last `spk up` snapshot.
///
/// Returns Skip when the daemon has never been started (no vm-config.json).
pub fn check_vm_resources(speck_home: &Path) -> CheckResult {
    match crate::commands::up::read_vm_resource_snapshot(speck_home) {
        Ok(Some(cfg)) => CheckResult::PassWithDetail {
            detail: format!(
                "cpus: {}, memory: {} MiB, disk: {} GiB",
                cfg.cpus, cfg.memory_mb, cfg.disk_gb
            ),
        },
        Ok(None) => CheckResult::Skip {
            reason: "vm-config.json not found (daemon not started yet)".into(),
        },
        Err(e) => CheckResult::Warn {
            detail: format!("cannot read vm-config.json: {e}"),
        },
    }
}

/// Verify public DNS works by resolving `google.com:443` via `getaddrinfo`.
///
/// This uses the same code path as the in-guest DNS proxy so it confirms the host
/// resolver is functional.
pub fn check_public_dns() -> CheckResult {
    use std::net::ToSocketAddrs;
    let resolved = "google.com:443"
        .to_socket_addrs()
        .map(|addrs| addrs.count() > 0)
        .unwrap_or(false);
    if resolved {
        CheckResult::Pass
    } else {
        CheckResult::Fail {
            hint: "public DNS resolution failed — check network connectivity".into(),
        }
    }
}

/// Check for VPN-injected DNS resolvers via a one-shot SCDynamicStore read.
///
/// Returns Skip when no VPN is active (no supplemental-match-domain entries in
/// SCDynamicStore). Returns Pass when VPN-scoped entries are present, confirming
/// split-DNS data is available to the host resolver proxy.
pub fn check_vpn_dns() -> CheckResult {
    let table = speck_net::read_resolver_table_once();
    if table.is_empty() {
        CheckResult::Skip {
            reason: "no VPN active — connect a VPN to verify split-DNS inheritance".into(),
        }
    } else {
        CheckResult::Pass
    }
}

/// Check whether the Speck VM daemon is running by probing the control socket.
///
/// The daemon's control server reads first (PREPARE_RESTART protocol), so the
/// client writes PING — any unknown command elicits PONG from the `_ =>` arm —
/// and then drains the PONG reply under a 2-second timeout to avoid a
/// broken-pipe warning in the daemon log. Same pattern as `commands/down.rs`.
async fn check_vm_running(speck_home: &Path) -> CheckResult {
    let sock_path = speck_home.join("run/control.sock");
    match tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::UnixStream::connect(&sock_path),
    )
    .await
    {
        Ok(Ok(mut stream)) => {
            // Write PING first — the daemon reads before replying. Liveness is
            // defined by connect success; write/read outcomes are ignored.
            let _ = stream.write_all(b"PING\n").await;
            let mut buf = [0u8; 8];
            let _ = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
            CheckResult::Pass
        }
        _ => CheckResult::Fail {
            hint: "VM is not running — start with `spk up`".into(),
        },
    }
}

/// Verify the Docker-compatible API socket is reachable via GET /_ping.
///
/// Uses a 2-second timeout so a non-responsive daemon does not hang `spk doctor`.
async fn check_docker_socket(speck_home: &Path) -> CheckResult {
    let client = DockerClient::new(speck_home.join("speck.sock"));
    match tokio::time::timeout(Duration::from_secs(2), client.get_raw("/_ping")).await {
        Ok(Ok(_)) => CheckResult::Pass,
        _ => CheckResult::Fail {
            hint: "Docker socket unreachable — run `spk up` first".into(),
        },
    }
}

/// Version reported by the running daemon over the control socket, or
/// `None` when it is not running. Daemons older than 1.4.8 answer `PONG`.
async fn daemon_version(speck_home: &Path) -> Option<String> {
    let sock_path = speck_home.join("run/control.sock");
    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::UnixStream::connect(&sock_path),
    )
    .await
    .ok()?
    .ok()?;
    stream.write_all(b"VERSION\n").await.ok()?;
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
        .await
        .ok()?
        .ok()?;
    Some(String::from_utf8_lossy(&buf[..n]).trim().to_owned())
}

/// The running daemon must be this binary's version (OPS-03): after
/// `brew upgrade` the old daemon keeps running until `spk restart`.
fn daemon_version_result(reply: Option<&str>, current: &str) -> CheckResult {
    match reply {
        None => CheckResult::Skip {
            reason: "VM not running".into(),
        },
        Some(v) if v == current => CheckResult::PassWithDetail {
            detail: format!("spk {v}"),
        },
        Some(v) => {
            let running = if v == "PONG" {
                "an older spk that does not report its version".to_owned()
            } else {
                format!("spk {v}")
            };
            CheckResult::Warn {
                detail: format!(
                    "the daemon runs {running}, this is spk {current}: run `spk restart`"
                ),
            }
        }
    }
}

async fn check_daemon_version(speck_home: &Path) -> CheckResult {
    let reply = daemon_version(speck_home).await;
    daemon_version_result(reply.as_deref(), env!("CARGO_PKG_VERSION"))
}

/// Guest assets on disk must match this binary; a mismatch means the VM
/// still boots the old ones until the next start.
pub fn check_guest_assets(speck_home: &Path, assets: &Path) -> CheckResult {
    let drift = crate::commands::up::asset_drift(speck_home, assets);
    if drift.is_empty() {
        return CheckResult::Pass;
    }
    let list = drift
        .iter()
        .map(|(name, found, required)| format!("{name} {found} (needs {required})"))
        .collect::<Vec<_>>()
        .join(", ");
    CheckResult::Warn {
        detail: format!("{list}: run `spk restart` to update"),
    }
}

/// Seconds since the Unix epoch for an RFC 3339 time as dockerd prints it
/// (`2026-10-18T12:34:56.123456789Z` or with a `+02:00` offset).
fn parse_rfc3339(s: &str) -> Option<f64> {
    let s = s.trim();
    let num = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = s.get(19..)?;
    let tz_at = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let frac: f64 = rest[..tz_at]
        .strip_prefix('.')
        .map(|digits| format!("0.{digits}").parse().unwrap_or(0.0))
        .unwrap_or(0.0);
    let offset = match rest.get(tz_at..tz_at + 1) {
        Some(sign @ ("+" | "-")) => {
            let tz = rest.get(tz_at + 1..)?;
            let h: i64 = tz.get(0..2)?.parse().ok()?;
            let m: i64 = tz.get(3..5)?.parse().ok()?;
            let secs = h * 3600 + m * 60;
            if sign == "+" { secs } else { -secs }
        }
        _ => 0,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hour * 3600 + min * 60 + sec - offset) as f64 + frac)
}

/// Guest clock skew above this breaks TLS and AWS SigV4 sooner or later.
const MAX_CLOCK_SKEW_SECS: f64 = 2.0;

fn clock_result(guest: Option<f64>, host: f64) -> CheckResult {
    let Some(guest) = guest else {
        return CheckResult::Warn {
            detail: "dockerd did not report its system time".into(),
        };
    };
    let skew = guest - host;
    if skew.abs() <= MAX_CLOCK_SKEW_SECS {
        CheckResult::PassWithDetail {
            detail: format!("guest clock within {:.1}s of the host", skew.abs()),
        }
    } else {
        CheckResult::Warn {
            detail: format!(
                "guest clock is {:.0}s {} the host; TLS and AWS signatures may fail: run `spk restart`",
                skew.abs(),
                if skew > 0.0 { "ahead of" } else { "behind" }
            ),
        }
    }
}

/// Compare the guest clock (dockerd `/info` SystemTime) with the host.
async fn check_guest_clock(speck_home: &Path) -> CheckResult {
    let client = DockerClient::new(speck_home.join("speck.sock"));
    let info = match tokio::time::timeout(Duration::from_secs(3), client.get("/info")).await {
        Ok(Ok(info)) => info,
        _ => {
            return CheckResult::Skip {
                reason: "Docker socket unreachable".into(),
            };
        }
    };
    let host = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default();
    let guest = info["SystemTime"].as_str().and_then(parse_rfc3339);
    clock_result(guest, host)
}

/// Run `command` in the guest over `spk ssh`, with a timeout. `None` when
/// ssh is not available (VM stopped, old rootfs, no key).
async fn guest_exec(command: &str) -> Option<(i32, String)> {
    let profile = crate::profile::current();
    let home = &profile.home;
    std::os::unix::net::UnixStream::connect(crate::commands::ssh::ssh_socket(home)).ok()?;
    if !crate::commands::ssh::identity_file(home).exists() {
        return None;
    }
    let spk = crate::commands::ssh::spk_path().ok()?;
    let mut argv = vec!["-o".to_owned(), "BatchMode=yes".to_owned()];
    argv.extend(crate::commands::ssh::ssh_argv(
        &profile,
        &spk,
        &[command.to_owned()],
    ));
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new("ssh")
            .args(&argv)
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let code = output.status.code()?;
    // 255: ssh itself failed (auth, connection), not the command.
    (code != 255).then(|| (code, String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Base64 bodies of the PEM certificates in `pem`, whitespace removed.
fn pem_bodies(pem: &str) -> Vec<String> {
    pem.split("-----BEGIN CERTIFICATE-----")
        .skip(1)
        .filter_map(|rest| rest.split("-----END CERTIFICATE-----").next())
        .map(|body| body.split_whitespace().collect())
        .collect()
}

/// Staged certificates missing from the guest bundle, by file name.
fn missing_certs(staged: &[(String, String)], guest_bundle: &str) -> Vec<String> {
    let guest: std::collections::HashSet<String> = pem_bodies(guest_bundle).into_iter().collect();
    staged
        .iter()
        .filter(|(_, pem)| pem_bodies(pem).iter().any(|body| !guest.contains(body)))
        .map(|(name, _)| name.clone())
        .collect()
}

/// The staged corporate CAs (config + Keychain) must be in the guest trust
/// store, or `docker pull` fails behind TLS inspection (OPS-03b).
async fn check_guest_ca(speck_home: &Path) -> CheckResult {
    let staged: Vec<(String, String)> = std::fs::read_dir(speck_home.join("ca-certs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "pem"))
        .filter_map(|e| {
            let pem = std::fs::read_to_string(e.path()).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), pem))
        })
        .collect();
    if staged.is_empty() {
        return CheckResult::Skip {
            reason: "no corporate CA certificates staged".into(),
        };
    }
    let Some((0, bundle)) = guest_exec("cat /etc/ssl/certs/ca-certificates.crt").await else {
        return CheckResult::Skip {
            reason: "guest not reachable over `spk ssh`".into(),
        };
    };
    let missing = missing_certs(&staged, &bundle);
    if missing.is_empty() {
        CheckResult::PassWithDetail {
            detail: format!(
                "{} staged certificate file(s) trusted in the guest",
                staged.len()
            ),
        }
    } else {
        CheckResult::Fail {
            hint: format!(
                "not in the guest trust store: {}; run `spk restart`",
                missing.join(", ")
            ),
        }
    }
}

/// TCP egress from the guest itself (the path `docker pull` takes).
async fn check_guest_egress() -> CheckResult {
    match guest_exec("nc -z -w5 registry-1.docker.io 443").await {
        None => CheckResult::Skip {
            reason: "guest not reachable over `spk ssh`".into(),
        },
        Some((0, _)) => CheckResult::PassWithDetail {
            detail: "guest reaches registry-1.docker.io:443".into(),
        },
        Some(_) => CheckResult::Fail {
            hint: "the guest cannot reach registry-1.docker.io:443; check the network or VPN, \
                   then `spk logs`"
                .into(),
        },
    }
}

// ──────────────────────────────────────────────────────────────
// DNS full-path trace (DOCTOR-03)
// ──────────────────────────────────────────────────────────────

/// Result of a DNS trace using the host resolver proxy path.
#[derive(Debug, Default)]
pub struct DnsTraceResult {
    /// Nameserver(s) that will handle the query — VPN-scoped servers when split-DNS
    /// is active, otherwise "(macOS system resolver)" as a display placeholder.
    pub nameserver: Option<String>,
    /// True when the domain matched a VPN-scoped split-DNS entry.
    pub vpn_scoped: bool,
    /// IPv4 answer addresses returned by the resolver.
    pub addrs: Vec<String>,
    /// Error string when resolution failed (NXDOMAIN, timeout, etc.).
    pub error: Option<String>,
}

/// Resolve `hostname` using the same path as the guest vsock DNS proxy.
///
/// Path B (DOCTOR-03): the guest's DNS forwarder proxies all UDP:53 queries over
/// vsock to the host, where `spawn_dns_proxy` calls `getaddrinfo`. This function
/// exercises that exact in-process path — no Alpine container or Docker socket needed,
/// and no network connectivity to the guest is required.
///
/// # Security (T-13-01)
/// Validates `hostname` against shell metacharacters and whitespace before any
/// further processing, satisfying ASVS V5 input validation requirements.
fn trace_dns(hostname: &str) -> anyhow::Result<DnsTraceResult> {
    use std::net::ToSocketAddrs;

    // Validate hostname (T-13-01 ASVS V5 input validation).
    if hostname
        .chars()
        .any(|c| c.is_whitespace() || ";|&$`'\"\\".contains(c))
    {
        anyhow::bail!("invalid hostname: contains shell metacharacters or whitespace");
    }

    let table = speck_net::read_resolver_table_once();
    let (nameserver, vpn_scoped) = match table.find_resolver(hostname) {
        Some(servers) => {
            let ns = servers
                .iter()
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            (Some(ns), true)
        }
        None => (Some("macOS system resolver".to_owned()), false),
    };

    // Resolve via getaddrinfo — same code path as spawn_dns_proxy in speck-net.
    let mut result = DnsTraceResult {
        nameserver,
        vpn_scoped,
        ..Default::default()
    };
    match format!("{hostname}:0").to_socket_addrs() {
        Ok(addrs) => {
            result.addrs = addrs
                .filter_map(|sa| match sa {
                    std::net::SocketAddr::V4(v4) => Some(v4.ip().to_string()),
                    _ => None,
                })
                .collect();
        }
        Err(e) => {
            result.error = Some(e.to_string());
        }
    }
    Ok(result)
}

/// Entry point for `spk doctor dns <hostname>`.
///
/// Resolves `hostname` using the same `getaddrinfo` path as the guest vsock DNS
/// proxy. Prints the nameserver path, answer IPs, and a PASS/FAIL verdict.
pub async fn run_doctor_dns(_speck_home: &Path, hostname: &str) -> anyhow::Result<i32> {
    let result = trace_dns(hostname)?;

    println!("DNS trace for: {hostname}\n");

    match &result.nameserver {
        Some(ns) if result.vpn_scoped => println!("  Nameserver: {ns}  (VPN split-DNS)"),
        Some(ns) => println!("  Nameserver: {ns}"),
        None => println!("  Nameserver: (unknown)"),
    }

    if let Some(err) = &result.error {
        println!();
        println!(
            "  {RED}FAIL{RESET}  resolution failed: {err}\n         hint: check network connectivity or VPN DNS configuration"
        );
        return Ok(1);
    }

    for addr in &result.addrs {
        println!("  Address:    {addr}");
    }

    println!();
    if result.addrs.is_empty() {
        println!("  {YELLOW}WARN{RESET}  resolver returned no addresses for {hostname}");
        Ok(0)
    } else {
        println!(
            "  {GREEN}PASS{RESET}  host resolver working — guest DNS inherits this path via vsock proxy"
        );
        Ok(0)
    }
}

/// Entry point for `spk doctor`.
///
/// Runs the health checks in `run_checks` and prints PASS/FAIL/WARN/SKIP
/// for each. Returns exit code 0 when all checks are Pass/Warn/Skip, 1 when any is Fail.
///
/// Check order: codesign entitlement, DOCKER_HOST, cert injection, VM resources,
/// public DNS, VPN DNS, VM running, Docker socket, daemon version, guest
/// assets, guest clock, guest CA, guest egress.
pub async fn run_doctor(
    speck_home: &Path,
    args: DoctorArgs,
    format: Output,
) -> anyhow::Result<i32> {
    if let Some(DoctorSubcommand::Dns { hostname }) = args.command {
        anyhow::ensure!(
            format.is_plain(),
            "`spk doctor dns` supports only plain output"
        );
        return run_doctor_dns(speck_home, &hostname).await;
    }

    let checks = run_checks(speck_home).await;
    let any_fail = checks
        .iter()
        .any(|(_, r)| matches!(r, CheckResult::Fail { .. }));
    if !format.is_plain() {
        output::emit(
            format,
            &DoctorDoc {
                speck_home: speck_home.display().to_string(),
                ok: !any_fail,
                checks: checks.iter().map(|(l, r)| CheckDoc::new(l, r)).collect(),
            },
        )?;
        return Ok(if any_fail { 1 } else { 0 });
    }

    println!("Speck Doctor\n");

    let speck_home_note = if std::env::var("SPECK_HOME").is_ok() {
        " (from $SPECK_HOME env)"
    } else {
        ""
    };
    println!("  SPECK_HOME: {}{speck_home_note}", speck_home.display());
    println!();

    for (label, result) in &checks {
        print_check(label, result);
    }

    if any_fail { Ok(1) } else { Ok(0) }
}

async fn run_checks(speck_home: &Path) -> Vec<(&'static str, CheckResult)> {
    vec![
        ("codesign entitlement", check_codesign()),
        ("DOCKER_HOST", check_docker_host(speck_home)),
        ("cert injection", check_cert_injection(speck_home)),
        ("VM resources", check_vm_resources(speck_home)),
        ("public DNS", check_public_dns()),
        ("VPN DNS", check_vpn_dns()),
        ("VM running", check_vm_running(speck_home).await),
        ("Docker socket", check_docker_socket(speck_home).await),
        ("daemon version", check_daemon_version(speck_home).await),
        (
            "guest assets",
            check_guest_assets(speck_home, &crate::profile::current().assets),
        ),
        ("guest clock", check_guest_clock(speck_home).await),
        ("guest CA", check_guest_ca(speck_home).await),
        ("guest egress", check_guest_egress().await),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_certs_missing_from_the_guest_bundle_are_listed() {
        let a = "-----BEGIN CERTIFICATE-----\nAAAA\nBBBB\n-----END CERTIFICATE-----\n";
        let b = "-----BEGIN CERTIFICATE-----\nCCCC\n-----END CERTIFICATE-----\n";
        let staged = vec![
            ("a.pem".to_owned(), a.to_owned()),
            ("b.pem".to_owned(), b.to_owned()),
        ];
        // Line wrapping differs between the staged file and the bundle.
        let bundle = "# roots\n-----BEGIN CERTIFICATE-----\nAAAABBBB\n-----END CERTIFICATE-----\n";
        assert_eq!(missing_certs(&staged, bundle), ["b.pem"]);
        assert!(missing_certs(&staged[..1], bundle).is_empty());
    }

    #[test]
    fn rfc3339_parses_dockerd_system_time() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_rfc3339("2026-10-18T12:00:00Z"), Some(1_792_324_800.0));
        assert_eq!(
            parse_rfc3339("2026-10-18T14:00:00.5+02:00"),
            Some(1_792_324_800.5)
        );
        assert_eq!(
            parse_rfc3339("2026-10-18T12:00:00.250000000Z"),
            Some(1_792_324_800.25)
        );
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn clock_skew_over_two_seconds_warns() {
        assert!(matches!(
            clock_result(Some(101.5), 100.0),
            CheckResult::PassWithDetail { .. }
        ));
        let CheckResult::Warn { detail } = clock_result(Some(40.0), 100.0) else {
            panic!("expected a warning");
        };
        assert!(detail.contains("60s behind"), "{detail}");
        assert!(matches!(
            clock_result(None, 100.0),
            CheckResult::Warn { .. }
        ));
    }

    #[test]
    fn daemon_version_mismatch_warns() {
        assert!(matches!(
            daemon_version_result(None, "1.4.8"),
            CheckResult::Skip { .. }
        ));
        assert!(matches!(
            daemon_version_result(Some("1.4.8"), "1.4.8"),
            CheckResult::PassWithDetail { .. }
        ));
        let CheckResult::Warn { detail } = daemon_version_result(Some("PONG"), "1.4.8") else {
            panic!("expected a warning");
        };
        assert!(detail.contains("older spk"), "{detail}");
        let CheckResult::Warn { detail } = daemon_version_result(Some("1.4.6"), "1.4.8") else {
            panic!("expected a warning");
        };
        assert!(detail.contains("spk 1.4.6"), "{detail}");
    }

    #[test]
    fn guest_assets_report_drift() {
        let dir = std::env::temp_dir().join(format!("speck-doctor-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("initrd")).unwrap();
        assert_eq!(check_guest_assets(&dir, &dir), CheckResult::Pass);
        std::fs::write(dir.join("initrd/initrd.version"), "0.0.1\n").unwrap();
        let CheckResult::Warn { detail } = check_guest_assets(&dir, &dir) else {
            panic!("expected a warning");
        };
        assert!(detail.starts_with("initrd 0.0.1 (needs "), "{detail}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dns_trace_accepts_hostnames_with_n_and_rejects_metacharacters() {
        for bad in ["a b", "a;b", "a\nb", "a\\b", "a$b", "a`b"] {
            assert!(super::trace_dns(bad).is_err(), "{bad:?} must be rejected");
        }
        let err = super::trace_dns("nonexistent.invalid")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(!err.contains("metacharacters"), "{err}");
    }

    const DOCTOR_SOURCE: &str = include_str!("doctor.rs");

    /// Slice production code only — everything before `#[cfg(test)]` — so tests do not
    /// trivially pass because assertion strings themselves contain the searched tokens.
    fn production_code() -> &'static str {
        let end = DOCTOR_SOURCE
            .find("#[cfg(test)]")
            .unwrap_or(DOCTOR_SOURCE.len());
        &DOCTOR_SOURCE[..end]
    }

    #[test]
    fn all_pass_exits_0() {
        let src = production_code();
        assert!(
            src.contains("any_fail"),
            "run_doctor must track any_fail to determine exit code"
        );
        assert!(
            src.contains("Ok(0)"),
            "run_doctor must return Ok(0) when no checks fail"
        );
    }

    #[test]
    fn all_fail_exits_1() {
        let src = production_code();
        assert!(
            src.contains("CheckResult::Fail"),
            "run_doctor must recognise CheckResult::Fail variants"
        );
        assert!(
            src.contains("Ok(1)"),
            "run_doctor must return Ok(1) when any check fails"
        );
    }

    #[test]
    fn docker_host_conflict_is_warn() {
        let src = production_code();
        assert!(
            src.contains("DOCKER_HOST"),
            "check_docker_host must reference DOCKER_HOST env var"
        );
        assert!(
            src.contains("Warn"),
            "check_docker_host conflict must produce Warn, not Fail"
        );
    }

    #[test]
    fn cert_check_passes_when_no_certs() {
        let src = production_code();
        assert!(
            src.contains("no CA certs configured"),
            "cert check must pass with 'no CA certs configured' detail when extra_certs is empty"
        );
        assert!(
            src.contains("PassWithDetail"),
            "cert check must use PassWithDetail (not Skip) for the no-certs case"
        );
    }

    #[test]
    fn cert_check_fails_when_not_staged() {
        let src = production_code();
        assert!(
            src.contains("not staged"),
            "cert check must fail with 'not staged' hint when cert file is absent from ca-certs dir"
        );
    }

    #[test]
    fn vm_resources_skip_when_no_snapshot() {
        let src = production_code();
        assert!(
            src.contains("vm-config.json not found"),
            "vm resources check must skip with 'vm-config.json not found' when daemon has not started"
        );
    }

    #[test]
    fn check_vm_running_probes_control_socket() {
        let src = production_code();
        assert!(
            src.contains("async fn check_vm_running"),
            "check_vm_running must be an async function"
        );
        assert!(
            src.contains("control.sock"),
            "check_vm_running must connect to control.sock"
        );
        assert!(
            src.contains("Duration::from_secs(2)"),
            "check_vm_running must use a 2-second timeout"
        );
    }

    #[test]
    fn check_vm_running_writes_ping_before_read() {
        let src = production_code();
        let ping_pos = src
            .find("write_all(b\"PING")
            .expect("check_vm_running must write PING before reading (daemon reads first)");
        let read_pos = src
            .find("stream.read")
            .expect("check_vm_running must read the PONG reply");
        assert!(
            ping_pos < read_pos,
            "PING write must precede the read — the daemon's control server reads first"
        );
        let after_ping = &src[ping_pos..];
        assert!(
            after_ping.contains("timeout(Duration::from_secs(2), stream.read"),
            "check_vm_running read must be wrapped in a 2-second tokio::time::timeout"
        );
        assert!(
            !src.contains("the PONG the daemon writes on connection"),
            "stale doc comment claiming the daemon writes PONG on connection must be gone"
        );
    }

    #[test]
    fn check_docker_socket_pings_api() {
        let src = production_code();
        assert!(
            src.contains("async fn check_docker_socket"),
            "check_docker_socket must be an async function"
        );
        assert!(
            src.contains("/_ping"),
            "check_docker_socket must call /_ping"
        );
    }

    #[test]
    fn run_doctor_has_the_daemon_and_guest_checks() {
        let src = production_code();
        for label in [
            "\"daemon version\"",
            "\"guest assets\"",
            "\"guest clock\"",
            "\"guest CA\"",
            "\"guest egress\"",
        ] {
            assert!(src.contains(label), "run_checks must include {label}");
        }
        assert!(
            src.contains("\"VM running\""),
            "run_doctor must include a 'VM running' check label"
        );
        assert!(
            src.contains("\"Docker socket\""),
            "run_doctor must include a 'Docker socket' check label"
        );
    }

    #[test]
    fn dns_trace_uses_host_resolver_path() {
        let src = production_code();
        assert!(
            src.contains("to_socket_addrs"),
            "trace_dns must resolve via getaddrinfo (to_socket_addrs) — same path as vsock DNS proxy"
        );
        assert!(
            src.contains("shell metacharacters"),
            "trace_dns must validate hostname before any resolution"
        );
        assert!(
            !src.contains("bash -c"),
            "trace_dns must never spawn bash -c with user-supplied hostname"
        );
        assert!(
            !src.contains("alpine"),
            "trace_dns must not depend on an Alpine container (Path B)"
        );
    }
}
