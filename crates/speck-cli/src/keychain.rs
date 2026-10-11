//! Export of admin-trusted certificates from the macOS Keychain (TLS-02).
//!
//! Corporate TLS-inspection roots (Cloudflare WARP, Zscaler, MDM-pushed CAs)
//! are installed with admin-domain trust settings. Mirroring exactly those
//! into the guest keeps "the VM trusts what the host trusts" without the user
//! locating and listing PEM files by hand. Apple's built-in roots are not
//! exported: the guest already ships the Mozilla root store.

use std::collections::HashSet;
use std::process::Command;

/// PEMs of all certificates with admin-domain trust settings that trust them.
/// Returns an empty list when the `security` tool fails; this never blocks
/// `spk up`.
pub fn admin_trusted_pems() -> Vec<String> {
    let export =
        std::env::temp_dir().join(format!("speck-admin-trust-{}.plist", std::process::id()));
    let exported = Command::new("security")
        .args(["trust-settings-export", "-d"])
        .arg(&export)
        .output()
        .is_ok_and(|o| o.status.success());
    let xml = if exported {
        std::fs::read_to_string(&export).unwrap_or_default()
    } else {
        String::new()
    };
    let _ = std::fs::remove_file(&export);
    let trusted = trusted_hashes(&xml);
    if trusted.is_empty() {
        return Vec::new();
    }

    let Ok(listing) = Command::new("security")
        .args(["find-certificate", "-a", "-Z", "-p"])
        .output()
    else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    pems_by_sha1(&String::from_utf8_lossy(&listing.stdout))
        .into_iter()
        .filter(|(sha1, _)| trusted.contains(sha1) && seen.insert(sha1.clone()))
        .map(|(_, pem)| pem)
        .collect()
}

/// SHA-1 hashes (upper-case hex) of certificates that an exported trust
/// settings plist marks as trusted.
///
/// An entry with no `kSecTrustSettingsResult` (empty settings) means "always
/// trust as root". Otherwise it counts as trusted when some setting is
/// TrustRoot (1) or TrustAsRoot (2) and none is Deny (3).
fn trusted_hashes(xml: &str) -> HashSet<String> {
    let mut results: Vec<(String, Vec<i64>)> = Vec::new();
    let mut lines = xml.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if let Some(key) = line
            .strip_prefix("<key>")
            .and_then(|l| l.strip_suffix("</key>"))
        {
            if key.len() == 40 && key.chars().all(|c| c.is_ascii_hexdigit()) {
                results.push((key.to_ascii_uppercase(), Vec::new()));
            } else if key == "kSecTrustSettingsResult" {
                let value = lines
                    .next()
                    .and_then(|l| l.strip_prefix("<integer>"))
                    .and_then(|l| l.strip_suffix("</integer>"))
                    .and_then(|v| v.parse().ok());
                if let (Some(v), Some((_, r))) = (value, results.last_mut()) {
                    r.push(v);
                }
            }
        }
    }
    results
        .into_iter()
        .filter(|(_, r)| r.is_empty() || (r.iter().any(|v| *v == 1 || *v == 2) && !r.contains(&3)))
        .map(|(hash, _)| hash)
        .collect()
}

/// Pair each PEM block in `security find-certificate -Z -p` output with the
/// SHA-1 hash printed above it.
fn pems_by_sha1(output: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut sha1: Option<String> = None;
    let mut pem: Option<String> = None;
    for line in output.lines() {
        if let Some(hash) = line.strip_prefix("SHA-1 hash: ") {
            sha1 = Some(hash.trim().to_ascii_uppercase());
        } else if line.starts_with("-----BEGIN CERTIFICATE-----") {
            pem = Some(format!("{line}\n"));
        } else if let Some(p) = pem.as_mut() {
            p.push_str(line);
            p.push('\n');
            if line.starts_with("-----END CERTIFICATE-----")
                && let (Some(h), Some(p)) = (sha1.take(), pem.take())
            {
                out.push((h, p));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>trustList</key>
	<dict>
		<key>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA</key>
		<dict>
			<key>trustSettings</key>
			<array/>
		</dict>
		<key>bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb</key>
		<dict>
			<key>trustSettings</key>
			<array>
				<dict>
					<key>kSecTrustSettingsResult</key>
					<integer>1</integer>
				</dict>
			</array>
		</dict>
		<key>CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC</key>
		<dict>
			<key>trustSettings</key>
			<array>
				<dict>
					<key>kSecTrustSettingsResult</key>
					<integer>3</integer>
				</dict>
			</array>
		</dict>
		<key>DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD</key>
		<dict>
			<key>trustSettings</key>
			<array>
				<dict>
					<key>kSecTrustSettingsResult</key>
					<integer>4</integer>
				</dict>
			</array>
		</dict>
	</dict>
	<key>trustVersion</key>
	<integer>1</integer>
</dict>
</plist>"#;

    #[test]
    fn trusted_hashes_follow_trust_results() {
        let t = trusted_hashes(EXPORT);
        assert!(t.contains("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")); // empty = always trust
        assert!(t.contains("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB")); // TrustRoot
        assert!(!t.contains("CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC")); // Deny
        assert!(!t.contains("DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD")); // Unspecified
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn trusted_hashes_of_empty_input_is_empty() {
        assert!(trusted_hashes("").is_empty());
    }

    #[test]
    fn pems_are_paired_with_their_sha1() {
        let listing = "SHA-256 hash: 00\nSHA-1 hash: aaaa\n-----BEGIN CERTIFICATE-----\nMIIA\n-----END CERTIFICATE-----\n\
                       SHA-256 hash: 11\nSHA-1 hash: BBBB\n-----BEGIN CERTIFICATE-----\nMIIB\nMIIC\n-----END CERTIFICATE-----\n";
        let pems = pems_by_sha1(listing);
        assert_eq!(pems.len(), 2);
        assert_eq!(pems[0].0, "AAAA");
        assert_eq!(
            pems[1].1,
            "-----BEGIN CERTIFICATE-----\nMIIB\nMIIC\n-----END CERTIFICATE-----\n"
        );
    }
}
