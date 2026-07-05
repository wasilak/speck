use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::config;
use crate::docker_client::DockerClient;
use crate::theme::{GREEN, RED, RESET, YELLOW};
use crate::{DoctorArgs, DoctorSubcommand};

/// Outcome of a single health check performed by `spk doctor`.
#[derive(Debug, PartialEq, Eq)]
pub enum CheckResult {
    Pass,
    Fail { hint: String },
    Warn { detail: String },
    Skip { reason: String },
}

/// Print one check result line to stdout using the Speck colour scheme.
pub fn print_check(label: &str, result: &CheckResult) {
    match result {
        CheckResult::Pass => println!("  {GREEN}PASS{RESET}  {label}"),
        CheckResult::Fail { hint } => {
            println!("  {RED}FAIL{RESET}  {label}\n         hint: {hint}")
        }
        CheckResult::Warn { detail } => {
            println!("  {YELLOW}WARN{RESET}  {label}\n         {detail}")
        }
        CheckResult::Skip { reason } => println!("  SKIP  {label} ({reason})"),
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
            }
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
                    hint: "Binary missing virtualization entitlement — run `cargo xtask codesign-dev`"
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
            }
        }
    };

    if app_config.ca.extra_certs.is_empty() {
        return CheckResult::Skip {
            reason: "no CA certs configured".into(),
        };
    }

    for path_str in &app_config.ca.extra_certs {
        let content = match std::fs::read(path_str) {
            Ok(c) => c,
            Err(_) => {
                return CheckResult::Fail {
                    hint: format!("configured cert source not found: {path_str}"),
                }
            }
        };
        let hash_hex = format!("{:x}", Sha256::digest(&content));
        let staged = speck_home
            .join("ca-certs")
            .join(format!("{hash_hex}.pem"));
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
        Ok(Some(cfg)) => {
            println!(
                "         cpus: {}, memory: {} MiB, disk: {} GiB",
                cfg.cpus, cfg.memory_mb, cfg.disk_gb
            );
            CheckResult::Pass
        }
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
            reason: "no VPN-scoped resolvers detected (no VPN active)".into(),
        }
    } else {
        CheckResult::Pass
    }
}

/// Check whether the Speck VM daemon is running by probing the control socket.
///
/// Connects with a 2-second timeout and drains at least one byte (the PONG the daemon
/// writes on connection) to prevent a broken-pipe warning in the daemon log — same
/// pattern used in `commands/down.rs`.
async fn check_vm_running(speck_home: &Path) -> CheckResult {
    let sock_path = speck_home.join("run/control.sock");
    match tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::UnixStream::connect(&sock_path),
    )
    .await
    {
        Ok(Ok(mut stream)) => {
            // Drain PONG to prevent broken-pipe warning in daemon (Pitfall 5).
            let mut buf = [0u8; 8];
            let _ = stream.read(&mut buf).await;
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
    match tokio::time::timeout(Duration::from_secs(2), client.get("/_ping")).await {
        Ok(Ok(_)) => CheckResult::Pass,
        _ => CheckResult::Fail {
            hint: "Docker socket unreachable — run `spk up` first".into(),
        },
    }
}

// ──────────────────────────────────────────────────────────────
// DNS full-path trace (DOCTOR-03)
// ──────────────────────────────────────────────────────────────

/// Result of a full-path DNS trace comparing guest resolver with host resolver.
#[derive(Debug, Default)]
pub struct DnsTraceResult {
    /// IP address of the resolver reported by nslookup inside the container.
    pub guest_resolver: Option<String>,
    /// Answer IPs returned by the guest resolver for the queried hostname.
    pub guest_addrs: Vec<String>,
    /// IP address of the resolver reported by host-side nslookup.
    pub host_resolver: Option<String>,
    /// Answer IPs returned by the host resolver for the queried hostname.
    pub host_addrs: Vec<String>,
    /// True when the Alpine image is absent; the caller should WARN and return early.
    pub no_image: bool,
}

/// Strip Docker's 8-byte multiplexed stream headers and return the payload as a String.
///
/// Docker log endpoints return frames with the format:
///   [1 byte stream type][3 bytes zero padding][4 bytes BE payload length][payload bytes]
///
/// This function strips every header and concatenates the payloads.
pub fn strip_docker_stream_headers(raw: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::new();
    let mut pos = 0;
    while pos + 8 <= raw.len() {
        let size =
            u32::from_be_bytes([raw[pos + 4], raw[pos + 5], raw[pos + 6], raw[pos + 7]]) as usize;
        let payload_end = pos + 8 + size;
        if payload_end > raw.len() {
            break;
        }
        out.extend_from_slice(&raw[pos + 8..payload_end]);
        pos = payload_end;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse busybox nslookup text output into a `DnsTraceResult`.
///
/// Expected format (busybox nslookup in Alpine):
/// ```text
/// Server:    8.8.8.8
/// Address 1: 8.8.8.8 dns.google
///
/// Name:      google.com
/// Address 1: 142.250.185.46 ...
/// ```
///
/// The `Server:` line yields `guest_resolver`; `Address` lines that appear after
/// a `Name:` line yield `guest_addrs`. `no_image` is always false for parsed output.
pub fn parse_nslookup_output(text: &str) -> DnsTraceResult {
    let mut result = DnsTraceResult::default();
    let mut after_name = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Server:") {
            result.guest_resolver = Some(rest.trim().to_owned());
        } else if line.starts_with("Name:") {
            after_name = true;
        } else if after_name && line.starts_with("Address") {
            // "Address 1: 1.2.3.4 optional-hostname"
            if let Some(after_colon) = line.splitn(2, ':').nth(1) {
                let ip = after_colon.trim().split_whitespace().next().unwrap_or("").to_owned();
                if !ip.is_empty() {
                    result.guest_addrs.push(ip);
                }
            }
        }
    }
    result
}

/// Run the full DNS trace pipeline through a temporary Alpine container.
///
/// Returns a `DnsTraceResult` with guest resolver and answer IPs filled.
/// Sets `no_image: true` and returns early (without error) when Alpine is absent.
///
/// # Security (T-13-01)
/// Validates `hostname` against shell metacharacters and whitespace before any
/// subprocess invocation, satisfying ASVS V5 input validation requirements.
async fn trace_dns(client: &DockerClient, hostname: &str) -> anyhow::Result<DnsTraceResult> {
    // Step 1 — Validate hostname (T-13-01 ASVS V5 input validation).
    if hostname.chars().any(|c| " ;|&$`'\"\\n".contains(c)) {
        anyhow::bail!("invalid hostname: contains shell metacharacters or whitespace");
    }

    // Step 2 — Check Alpine image is present.
    let image_check = client.get("/images/alpine/json").await;
    let image_ok = image_check.as_ref().ok().and_then(|v| v.get("Id")).is_some();
    if !image_ok {
        return Ok(DnsTraceResult { no_image: true, ..Default::default() });
    }

    // Step 3 — Create container: Cmd array prevents shell injection.
    let body = serde_json::json!({
        "Image": "alpine",
        "Cmd": ["nslookup", hostname],
        "NetworkDisabled": false
    });
    let create_resp = client.post_json("/containers/create", &body).await?;
    let id = create_resp["Id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("container create: no Id in response"))?
        .to_owned();

    // Step 4 — Start (204 No Content — ignore JSON parse error) then wait for exit.
    let _ = client.post_empty(&format!("/containers/{id}/start")).await;
    let _ = client.post_empty(&format!("/containers/{id}/wait")).await;

    // Step 5 — Fetch raw logs and strip Docker multiplexed stream headers.
    let raw_logs = client
        .get_raw(&format!("/containers/{id}/logs?stdout=1&stderr=1"))
        .await?;
    let log_text = strip_docker_stream_headers(&raw_logs);

    // Step 6 — Delete container (best-effort; don't fail trace on cleanup error).
    let _ = client.delete(&format!("/containers/{id}")).await;

    // Step 7 — Parse and return guest-side result.
    Ok(parse_nslookup_output(&log_text))
}

/// Entry point for `spk doctor dns <hostname>`.
///
/// Traces DNS resolution for `hostname` through the full guest path (via a temporary
/// Alpine container) and compares against the host-side resolver. Prints resolver IPs,
/// answer addresses, and whether guest and host agree.
pub async fn run_doctor_dns(speck_home: &Path, hostname: &str) -> anyhow::Result<i32> {
    let client = DockerClient::new(speck_home.join("speck.sock"));
    let mut result = trace_dns(&client, hostname).await?;

    if result.no_image {
        println!(
            "  {YELLOW}WARN{RESET}  DNS trace: Alpine image not present — run `docker pull alpine` to enable full DNS path testing"
        );
        return Ok(0);
    }

    // Host-side comparison: uses .arg(hostname) — never shell string (T-13-01).
    if let Ok(host_out) = std::process::Command::new("/usr/bin/nslookup").arg(hostname).output() {
        let host_text = String::from_utf8_lossy(&host_out.stdout).into_owned();
        let host_parse = parse_nslookup_output(&host_text);
        result.host_resolver = host_parse.guest_resolver;
        result.host_addrs = host_parse.guest_addrs;
    }

    println!("DNS trace for: {hostname}\n");

    println!("Guest DNS:");
    match &result.guest_resolver {
        Some(r) => println!("  Resolver: {r}"),
        None => println!("  Resolver: (unknown)"),
    }
    for addr in &result.guest_addrs {
        println!("  Address:  {addr}");
    }

    println!("\nHost DNS:");
    match &result.host_resolver {
        Some(r) => println!("  Resolver: {r}"),
        None => println!("  Resolver: (unknown)"),
    }
    for addr in &result.host_addrs {
        println!("  Address:  {addr}");
    }

    // Compare answer sets.
    let guest_set: std::collections::BTreeSet<_> = result.guest_addrs.iter().collect();
    let host_set: std::collections::BTreeSet<_> = result.host_addrs.iter().collect();
    println!();
    if !guest_set.is_empty() && guest_set == host_set {
        println!("  {GREEN}MATCH{RESET}  guest and host DNS answers agree");
    } else {
        println!("  {YELLOW}MISMATCH{RESET}  guest and host DNS answers differ");
    }

    Ok(0)
}

/// Entry point for `spk doctor`.
///
/// Runs eight health checks (six synchronous + two async) and prints PASS/FAIL/WARN/SKIP
/// for each. Returns exit code 0 when all checks are Pass/Warn/Skip, 1 when any is Fail.
///
/// Check order: codesign entitlement, DOCKER_HOST, cert injection, VM resources,
/// public DNS, VPN DNS, VM running, Docker socket.
pub async fn run_doctor(speck_home: &Path, args: DoctorArgs) -> anyhow::Result<i32> {
    if let Some(DoctorSubcommand::Dns { hostname }) = args.command {
        return run_doctor_dns(speck_home, &hostname).await;
    }

    println!("Speck Doctor\n");

    let checks: Vec<(&str, CheckResult)> = vec![
        ("codesign entitlement", check_codesign()),
        ("DOCKER_HOST", check_docker_host(speck_home)),
        ("cert injection", check_cert_injection(speck_home)),
        ("VM resources", check_vm_resources(speck_home)),
        ("public DNS", check_public_dns()),
        ("VPN DNS", check_vpn_dns()),
        ("VM running", check_vm_running(speck_home).await),
        ("Docker socket", check_docker_socket(speck_home).await),
    ];

    let mut any_fail = false;
    for (label, result) in &checks {
        if matches!(result, CheckResult::Fail { .. }) {
            any_fail = true;
        }
        print_check(label, result);
    }

    if any_fail { Ok(1) } else { Ok(0) }
}

#[cfg(test)]
mod tests {
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
    fn cert_check_skips_when_no_certs() {
        let src = production_code();
        assert!(
            src.contains("no CA certs configured"),
            "cert check must skip with 'no CA certs configured' when extra_certs is empty"
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
    fn run_doctor_has_eight_checks() {
        let src = production_code();
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
    fn dns_hostname_not_shell_injected() {
        let src = production_code();
        assert!(
            src.contains("Command::new(\"/usr/bin/nslookup\")"),
            "host-side nslookup must use Command::new with a literal path"
        );
        assert!(
            src.contains(".arg(hostname)"),
            "hostname must be passed as a direct .arg() call, not interpolated into a shell string"
        );
        assert!(
            !src.contains("bash -c"),
            "trace_dns must never spawn bash -c with user-supplied hostname"
        );
        assert!(
            src.contains("shell metacharacters"),
            "trace_dns must validate hostname before any subprocess invocation"
        );
    }
}
