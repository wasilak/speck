use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const VIRTUALIZATION_ENTITLEMENT: &str = "com.apple.security.virtualization";

pub(crate) fn task_dist() -> ExitCode {
    if std::env::args().nth(2).as_deref() == Some("--sign-only") {
        return task_sign_only();
    }

    println!("Checking ad-hoc development signing prerequisites before building artifacts...");
    match run_preflight() {
        Ok(()) => {
            eprintln!(
                "cargo xtask dist ad-hoc artifact creation for {} is handled by the next Phase 18 plan",
                release_binary_path().display()
            );
            ExitCode::from(1)
        }
        Err(failures) => {
            print_preflight_failures(&failures);
            ExitCode::from(1)
        }
    }
}

fn task_sign_only() -> ExitCode {
    if let Err(failures) = run_preflight() {
        print_preflight_failures(&failures);
        return ExitCode::from(1);
    }

    println!("Building Apple Silicon release binary...");
    if let Err(message) = build_release_binary() {
        eprintln!("release build FAILED: {message}");
        return ExitCode::from(1);
    }

    println!("Signing release binary with ad-hoc development identity...");
    if let Err(message) = sign_release_binary() {
        eprintln!("codesign FAILED: {message}");
        return ExitCode::from(1);
    }

    match signed_binary_has_virtualization_entitlement(release_binary_path()) {
        Ok(()) => {
            println!(
                "dist sign-only OK: {} is ad-hoc signed with boolean virtualization entitlement",
                release_binary_path().display()
            );
            ExitCode::from(0)
        }
        Err(message) => {
            eprintln!("entitlement validation FAILED: {message}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn task_dist_check() -> ExitCode {
    match run_preflight() {
        Ok(()) => {
            println!("dist-check OK: ad-hoc development signing prerequisites are available");
            ExitCode::from(0)
        }
        Err(failures) => {
            print_preflight_failures(&failures);
            ExitCode::from(1)
        }
    }
}

fn release_binary_path() -> &'static Path {
    Path::new("target/aarch64-apple-darwin/release/spk")
}

fn entitlements_path() -> &'static Path {
    Path::new("speck.entitlements")
}

fn pkg_payload_root() -> &'static Path {
    Path::new("packaging/pkg-root")
}

fn staged_binary_path() -> &'static Path {
    Path::new("packaging/pkg-root/usr/local/bin/spk")
}

fn archive_path(version: &str) -> PathBuf {
    PathBuf::from(format!(
        "dist/spk-{version}-aarch64-apple-darwin-development-non-notarized.tar.gz"
    ))
}

fn build_release_binary() -> Result<(), String> {
    run_command(
        Command::new("cargo").args([
            "build",
            "--release",
            "--package",
            "speck-cli",
            "--target",
            "aarch64-apple-darwin",
        ]),
    )
}

fn build_codesign_args(binary: &Path) -> Vec<&str> {
    vec![
        "--sign",
        "-",
        "--entitlements",
        "speck.entitlements",
        "--options",
        "runtime",
        "--force",
        binary.to_str().expect("release binary path must be UTF-8"),
    ]
}

fn sign_release_binary() -> Result<(), String> {
    let args = build_codesign_args(release_binary_path());
    run_command(Command::new("codesign").args(args))
}

fn signed_binary_has_virtualization_entitlement(binary: &Path) -> Result<(), String> {
    let output = Command::new("codesign")
        .args(["-d", "--entitlements", "-", binary.to_str().ok_or("non-UTF-8 binary path")?])
        .output()
        .map_err(|err| format!("failed to run codesign entitlement dump: {err}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }

    let plist = String::from_utf8_lossy(&output.stdout);
    if entitlement_plist_has_virtualization_true(&plist) {
        Ok(())
    } else {
        Err(format!(
            "{VIRTUALIZATION_ENTITLEMENT} must be present as boolean <true/> in signed artifact"
        ))
    }
}

fn run_command(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .map_err(|err| format!("failed to run {program}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

fn entitlement_plist_has_virtualization_true(plist: &str) -> bool {
    if !plist.contains("<plist") || !plist.contains("</plist>") || !plist.contains("<dict>") {
        return false;
    }

    let Some(key_start) = plist.find(&format!("<key>{VIRTUALIZATION_ENTITLEMENT}</key>")) else {
        return false;
    };
    let after_key = &plist[key_start + VIRTUALIZATION_ENTITLEMENT.len() + "<key></key>".len()..];
    let trimmed = after_key.trim_start();

    trimmed.starts_with("<true/>") || trimmed.starts_with("<true />")
}

#[allow(dead_code)]
fn notary_status_accepted(json: &str) -> bool {
    let trimmed = json.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return false;
    }

    json_string_value(trimmed, "status").is_some_and(|status| status == "Accepted")
}

#[allow(dead_code)]
fn json_string_value<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let key_start = json.find(&needle)?;
    let after_key = json[key_start + needle.len()..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let value = after_colon.strip_prefix('"')?;
    let value_end = value.find('"')?;
    Some(&value[..value_end])
}

fn run_preflight() -> Result<(), Vec<String>> {
    let mut failures = Vec::new();

    check_command(
        &mut failures,
        "codesign",
        "Install Xcode Command Line Tools: xcode-select --install",
    );

    let entitlements = entitlements_path();
    match std::fs::read_to_string(entitlements) {
        Ok(contents) if entitlement_plist_has_virtualization_true(&contents) => {}
        Ok(_) => failures.push(format!(
            "speck.entitlements must contain {VIRTUALIZATION_ENTITLEMENT} as boolean <true/>; fix speck.entitlements before signing"
        )),
        Err(_) => failures.push(
            "speck.entitlements is missing; restore the entitlement file before signing".to_string(),
        ),
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

fn check_command(failures: &mut Vec<String>, cmd: &str, next_step: &str) {
    match Command::new(cmd).arg("--help").output() {
        Ok(_) => {}
        _ => failures.push(format!(
            "missing Apple tool `{cmd}`; next step: {next_step}"
        )),
    }
}

fn print_preflight_failures(failures: &[String]) {
    eprintln!("dist-check FAILED: ad-hoc development signing prerequisites are incomplete");
    for failure in failures {
        eprintln!("- {failure}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dist_release_path_points_at_apple_silicon_release_spk() {
        assert_eq!(
            release_binary_path(),
            Path::new("target/aarch64-apple-darwin/release/spk")
        );
    }

    #[test]
    fn dist_entitlement_parser_requires_boolean_true() {
        let valid = r#"
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>com.apple.security.virtualization</key>
  <true/>
</dict>
</plist>
"#;

        assert!(entitlement_plist_has_virtualization_true(valid));
    }

    #[test]
    fn dist_entitlement_parser_rejects_string_true_missing_and_malformed() {
        let string_true = r#"
<plist version="1.0"><dict>
  <key>com.apple.security.virtualization</key>
  <string>true</string>
</dict></plist>
"#;
        let missing = r#"<plist version="1.0"><dict><key>other</key><true/></dict></plist>"#;
        let malformed =
            r#"<plist version="1.0"><dict><key>com.apple.security.virtualization</key>"#;

        assert!(!entitlement_plist_has_virtualization_true(string_true));
        assert!(!entitlement_plist_has_virtualization_true(missing));
        assert!(!entitlement_plist_has_virtualization_true(malformed));
    }

    #[test]
    fn dist_notary_status_parser_accepts_only_accepted() {
        assert!(notary_status_accepted(
            r#"{"id":"abc","status":"Accepted"}"#
        ));
        assert!(!notary_status_accepted(
            r#"{"id":"abc","status":"Invalid"}"#
        ));
        assert!(!notary_status_accepted(
            r#"{"id":"abc","status":"Rejected"}"#
        ));
        assert!(!notary_status_accepted(r#"{"id":"abc"}"#));
        assert!(!notary_status_accepted("not json"));
    }

    #[test]
    fn dist_preflight_failure_text_is_actionable() {
        let mut failures = Vec::new();
        check_command(&mut failures, "definitely-missing-speck-tool", "install it");

        assert!(failures.iter().any(|failure| {
            failure.contains("definitely-missing-speck-tool")
                && failure.contains("next step")
                && failure.contains("install it")
        }));
    }

    #[test]
    fn signing_command_uses_ad_hoc_identity_and_exact_entitlement_flags() {
        let command = build_codesign_args(release_binary_path());

        assert_eq!(command, vec![
            "--sign",
            "-",
            "--entitlements",
            "speck.entitlements",
            "--options",
            "runtime",
            "--force",
            "target/aarch64-apple-darwin/release/spk",
        ]);
        assert!(!command.contains(&"SPECK_DEVELOPER_ID_APPLICATION"));
        assert!(!command.contains(&"--timestamp"));
    }

    #[test]
    fn development_archive_name_is_explicitly_non_notarized() {
        assert_eq!(
            archive_path("0.1.0"),
            Path::new("dist/spk-0.1.0-aarch64-apple-darwin-development-non-notarized.tar.gz")
        );
    }

    #[test]
    fn package_stage_paths_install_to_usr_local_bin() {
        assert_eq!(pkg_payload_root(), Path::new("packaging/pkg-root"));
        assert_eq!(staged_binary_path(), Path::new("packaging/pkg-root/usr/local/bin/spk"));
    }
}
