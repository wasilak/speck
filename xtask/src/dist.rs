use std::path::Path;
use std::process::ExitCode;

const VIRTUALIZATION_ENTITLEMENT: &str = "com.apple.security.virtualization";

pub(crate) fn task_dist() -> ExitCode {
    eprintln!("cargo xtask dist is not implemented yet");
    ExitCode::from(1)
}

pub(crate) fn task_dist_check() -> ExitCode {
    eprintln!("cargo xtask dist-check is not implemented yet");
    ExitCode::from(1)
}

fn release_binary_path() -> &'static Path {
    Path::new("target/aarch64-apple-darwin/release/spk")
}

fn entitlement_plist_has_virtualization_true(_plist: &str) -> bool {
    false
}

fn notary_status_accepted(_json: &str) -> bool {
    false
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
        let malformed = r#"<plist version="1.0"><dict><key>com.apple.security.virtualization</key>"#;

        assert!(!entitlement_plist_has_virtualization_true(string_true));
        assert!(!entitlement_plist_has_virtualization_true(missing));
        assert!(!entitlement_plist_has_virtualization_true(malformed));
    }

    #[test]
    fn dist_notary_status_parser_accepts_only_accepted() {
        assert!(notary_status_accepted(r#"{"id":"abc","status":"Accepted"}"#));
        assert!(!notary_status_accepted(r#"{"id":"abc","status":"Invalid"}"#));
        assert!(!notary_status_accepted(r#"{"id":"abc","status":"Rejected"}"#));
        assert!(!notary_status_accepted(r#"{"id":"abc"}"#));
        assert!(!notary_status_accepted("not json"));
    }
}
