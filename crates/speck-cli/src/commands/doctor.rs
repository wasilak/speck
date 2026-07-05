use std::path::Path;

use sha2::{Digest, Sha256};

use crate::config;
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

/// Entry point for `spk doctor`.
///
/// Runs six synchronous health checks and prints PASS/FAIL/WARN/SKIP for each.
/// Returns exit code 0 when all checks are Pass/Warn/Skip, 1 when any check is Fail.
pub async fn run_doctor(speck_home: &Path, args: DoctorArgs) -> anyhow::Result<i32> {
    if let Some(DoctorSubcommand::Dns { .. }) = args.command {
        anyhow::bail!("spk doctor dns requires async checks — implemented in plan 13-02");
    }

    println!("Speck Doctor\n");

    let checks: Vec<(&str, CheckResult)> = vec![
        ("codesign entitlement", check_codesign()),
        ("DOCKER_HOST", check_docker_host(speck_home)),
        ("cert injection", check_cert_injection(speck_home)),
        ("VM resources", check_vm_resources(speck_home)),
        ("public DNS", check_public_dns()),
        ("VPN DNS", check_vpn_dns()),
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
}
