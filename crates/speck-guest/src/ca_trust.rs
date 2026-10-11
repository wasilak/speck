//! Guest trust store installation for host-provided CA certificates (CERT-02).
//!
//! Defined unconditionally (plain `std::fs`) so tests run on macOS.

use std::io::Write;
use std::path::Path;

/// Install every `.pem` file in `src` into the guest trust store.
///
/// Each file is copied into `local_dir` as `speck-<name>.crt`, the input
/// directory `update-ca-certificates` builds `bundle` from, and `update` runs
/// the tool. Every certificate still missing from `bundle` afterwards (tool
/// absent, failed, or ignoring the directory) is appended to `bundle`
/// directly, so trust never silently depends on the tool. Appending alone is
/// not enough: a later `update-ca-certificates` run regenerates `bundle` and
/// would drop anything that only lived there.
///
/// Returns the number of `.pem` files installed.
pub fn install_ca_certs(
    src: &Path,
    local_dir: &Path,
    bundle: &Path,
    update: impl FnOnce() -> bool,
) -> std::io::Result<usize> {
    let mut pems = Vec::new();
    for entry in std::fs::read_dir(src)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pem") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("ca");
        pems.push((format!("speck-{stem}.crt"), content));
    }
    if pems.is_empty() {
        return Ok(0);
    }

    std::fs::create_dir_all(local_dir)?;
    for (name, content) in &pems {
        std::fs::write(local_dir.join(name), content)?;
    }

    let _ = update();

    let current = std::fs::read_to_string(bundle).unwrap_or_default();
    let present = normalize(&current);
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(bundle)?;
    for (name, content) in &pems {
        let missing = cert_bodies(content)
            .iter()
            .any(|body| !present.contains(body.as_str()));
        if missing {
            writeln!(out, "\n# Speck injected CA: {name}")?;
            out.write_all(content.as_bytes())?;
            writeln!(out)?;
        }
    }
    Ok(pems.len())
}

/// Base64 bodies of the certificates in a PEM text, whitespace removed.
fn cert_bodies(pem: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    let mut current: Option<String> = None;
    for line in pem.lines() {
        let line = line.trim();
        if line == "-----BEGIN CERTIFICATE-----" {
            current = Some(String::new());
        } else if line == "-----END CERTIFICATE-----" {
            if let Some(body) = current.take() {
                bodies.push(body);
            }
        } else if let Some(body) = current.as_mut() {
            body.push_str(line);
        }
    }
    bodies
}

fn normalize(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const PEM: &str =
        "-----BEGIN CERTIFICATE-----\nMIIBcorpCAfirst\nlineTWO==\n-----END CERTIFICATE-----\n";

    fn dirs(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("speck-ca-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("corp.pem"), PEM).unwrap();
        std::fs::write(src.join("notes.txt"), "ignored").unwrap();
        (src, root.join("local"), root.join("bundle.crt"))
    }

    /// Simulates update-ca-certificates: regenerate the bundle from the
    /// system roots plus everything in the local directory.
    fn regenerate(local: &Path, bundle: &Path) -> bool {
        let mut out = String::from("SYSTEMROOTS\n");
        for e in std::fs::read_dir(local).unwrap().flatten() {
            out.push_str(&std::fs::read_to_string(e.path()).unwrap());
        }
        std::fs::write(bundle, out).unwrap();
        true
    }

    #[test]
    fn certs_survive_a_bundle_regeneration() {
        let (src, local, bundle) = dirs("regen");
        std::fs::write(&bundle, "SYSTEMROOTS\n").unwrap();

        let n = install_ca_certs(&src, &local, &bundle, || regenerate(&local, &bundle)).unwrap();
        assert_eq!(n, 1);
        assert!(local.join("speck-corp.crt").is_file());

        // A later run of the tool (e.g. a package hook) must not drop the CA.
        regenerate(&local, &bundle);
        let text = std::fs::read_to_string(&bundle).unwrap();
        assert!(normalize(&text).contains("MIIBcorpCAfirstlineTWO=="));
    }

    #[test]
    fn missing_tool_falls_back_to_appending_once() {
        let (src, local, bundle) = dirs("fallback");
        std::fs::write(&bundle, "SYSTEMROOTS\n").unwrap();

        install_ca_certs(&src, &local, &bundle, || false).unwrap();
        install_ca_certs(&src, &local, &bundle, || false).unwrap();

        let text = std::fs::read_to_string(&bundle).unwrap();
        assert_eq!(text.matches("MIIBcorpCAfirst").count(), 1);
    }

    #[test]
    fn empty_source_installs_nothing() {
        let root = std::env::temp_dir().join(format!("speck-ca-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let n = install_ca_certs(&root, &root.join("local"), &root.join("b"), || true).unwrap();
        assert_eq!(n, 0);
        assert!(!root.join("b").exists());
    }
}
