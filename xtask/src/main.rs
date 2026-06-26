use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let Some(subcommand) = std::env::args().nth(1) else {
        eprintln!("Usage: cargo xtask <command>");
        eprintln!("Commands:");
        eprintln!("  ci     Run all CI checks locally");
        eprintln!("  sign   Codesign the release binary with virtualization entitlement");
        eprintln!("  init   Download kernel + initrd for VM boot");
        return ExitCode::from(1);
    };

    match subcommand.as_str() {
        "ci" => task_ci(),
        "sign" => task_sign(),
        "init" => task_init(),
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!("Usage: cargo xtask <ci|sign|init>");
            ExitCode::from(1)
        }
    }
}

fn task_ci() -> ExitCode {
    let steps: &[(&str, &[&str], &str)] = &[
        ("cargo", &["fmt", "--all", "--", "--check"], "formatting"),
        (
            "cargo",
            &["clippy", "--all-targets", "--", "-Dwarnings"],
            "clippy",
        ),
    ];

    for (cmd, args, label) in steps {
        print!("Checking {label}... ");
        if run(cmd, args).is_err() {
            eprintln!("FAILED");
            return ExitCode::from(1);
        }
        println!("ok");
    }

    print!("Checking no-print in speck-core... ");
    if let Err(msg) = no_print_check() {
        eprintln!("FAILED");
        eprintln!("{msg}");
        return ExitCode::from(1);
    }
    println!("ok");

    print!("Running tests... ");
    if run("cargo", &["test", "--workspace"]).is_err() {
        eprintln!("FAILED");
        return ExitCode::from(1);
    }
    println!("ok");

    let release_binary = "target/aarch64-apple-darwin/release/spk";
    if std::path::Path::new(release_binary).exists() {
        print!("Verifying code signature... ");
        if run("codesign", &["--verify", "--verbose", release_binary]).is_err() {
            eprintln!("FAILED");
            eprintln!("  Binary exists but is not signed or signature invalid.");
            eprintln!("  Run: cargo xtask sign");
            return ExitCode::from(1);
        }
        println!("ok");
    }

    println!("All CI checks passed.");
    ExitCode::from(0)
}

fn task_init() -> ExitCode {
    let scripts: &[&str] = &["scripts/fetch-kernel.sh", "scripts/fetch-rootfs.sh"];

    for script_name in scripts {
        let script = std::path::Path::new(script_name);
        if !script.exists() {
            eprintln!("Script not found: {script_name}");
            return ExitCode::from(1);
        }

        println!("Running {script_name} ...");
        let status = std::process::Command::new("bash")
            .args([script])
            .status()
            .expect("failed to run {script_name}");

        if !status.success() {
            eprintln!("init FAILED at {script_name}");
            return ExitCode::from(1);
        }
    }

    ExitCode::from(0)
}

fn task_sign() -> ExitCode {
    let profile_arg = std::env::args().nth(2);
    let profile = profile_arg.as_deref().unwrap_or("release");
    let target_dir = format!("target/aarch64-apple-darwin/{profile}");

    let binary_name = if std::path::Path::new(&format!("{target_dir}/spk")).exists() {
        "spk"
    } else if std::path::Path::new(&format!("{target_dir}/speck-vz")).exists() {
        "speck-vz"
    } else {
        eprintln!("No binary found in {target_dir}");
        eprintln!("Build first: cargo build --profile {profile}");
        return ExitCode::from(1);
    };

    let binary_path = format!("{target_dir}/{binary_name}");
    let entitlements_path = "speck.entitlements";

    if !std::path::Path::new(entitlements_path).exists() {
        eprintln!("Entitlements file not found: {entitlements_path}");
        return ExitCode::from(1);
    }

    let identity = "-";

    let status = std::process::Command::new("codesign")
        .args([
            "--sign",
            identity,
            "--entitlements",
            entitlements_path,
            "--force",
            "--options",
            "runtime",
            "--timestamp",
            &binary_path,
        ])
        .status()
        .expect("failed to run codesign");

    if !status.success() {
        eprintln!("Signing FAILED");
        return ExitCode::from(1);
    }

    let verify = std::process::Command::new("codesign")
        .args(["--verify", "--verbose", "--entitlements", "-", &binary_path])
        .output()
        .expect("failed to verify codesign");

    if verify.status.success() {
        let stdout = String::from_utf8_lossy(&verify.stdout);
        println!("Signing OK");
        println!("{stdout}");
        ExitCode::from(0)
    } else {
        let stderr = String::from_utf8_lossy(&verify.stderr);
        eprintln!("Verification FAILED: {stderr}");
        ExitCode::from(1)
    }
}

fn no_print_check() -> Result<(), String> {
    let output = Command::new("rg")
        .args([
            "--type",
            "rust",
            r#"(print!|println!|eprint!|eprintln!)\s*\("#,
            "crates/speck-core/src/",
        ])
        .output()
        .map_err(|e| format!("Failed to run rg: {e} (is ripgrep installed?)"))?;

    match output.status.code() {
        Some(0) => {
            let matches = String::from_utf8_lossy(&output.stdout);
            Err(format!(
                "No-print lint failed: found print macros in speck-core:\n{matches}"
            ))
        }
        Some(1) => Ok(()),
        Some(2) => Err("rg encountered an error during the no-print check (exit code 2)".into()),
        _ => Err("rg terminated by signal".into()),
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<(), ()> {
    let status = Command::new(cmd).args(args).status().map_err(|_| ())?;
    if status.success() { Ok(()) } else { Err(()) }
}
