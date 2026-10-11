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
/// `local_dir` lives on the persistent guest rootfs, so `speck-*.crt` files
/// not in the current set (a CA removed from the host config or Keychain) are
/// deleted and the bundle rebuilt; a missing `src` counts as an empty set.
///
/// Returns the number of `.pem` files installed.
pub fn install_ca_certs(
    src: &Path,
    local_dir: &Path,
    bundle: &Path,
    update: impl FnOnce() -> bool,
) -> std::io::Result<usize> {
    let mut pems = Vec::new();
    for entry in std::fs::read_dir(src).into_iter().flatten().flatten() {
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

    let removed = remove_stale(local_dir, &pems);
    if pems.is_empty() {
        if removed > 0 {
            let _ = update();
        }
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

/// Delete `speck-*.crt` files in `local_dir` that are not in `pems`.
fn remove_stale(local_dir: &Path, pems: &[(String, String)]) -> usize {
    let mut removed = 0;
    for entry in std::fs::read_dir(local_dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with("speck-") && name.ends_with(".crt");
        if ours
            && !pems.iter().any(|(n, _)| *n == name)
            && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
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
    fn removed_cas_are_dropped_from_the_trust_store() {
        let (src, local, bundle) = dirs("revoke");
        std::fs::write(&bundle, "SYSTEMROOTS\n").unwrap();
        install_ca_certs(&src, &local, &bundle, || regenerate(&local, &bundle)).unwrap();
        std::fs::write(local.join("unrelated.crt"), "keep").unwrap();

        // The CA is no longer configured on the host; src is gone entirely.
        std::fs::remove_dir_all(&src).unwrap();
        let n = install_ca_certs(&src, &local, &bundle, || regenerate(&local, &bundle)).unwrap();

        assert_eq!(n, 0);
        assert!(!local.join("speck-corp.crt").exists());
        assert!(local.join("unrelated.crt").exists());
        let text = std::fs::read_to_string(&bundle).unwrap();
        assert!(!normalize(&text).contains("MIIBcorpCAfirst"));
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
