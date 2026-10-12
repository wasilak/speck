//! Running linux/amd64 binaries in the arm64 guest via Rosetta.
//!
//! The host shares Apple's Rosetta for Linux over VirtioFS (tag `rosetta`);
//! vminitd mounts it at [`ROSETTA_DIR`] and registers it in binfmt_misc for
//! x86_64 ELF executables.

/// Where vminitd mounts the Rosetta share.
pub const ROSETTA_DIR: &str = "/run/rosetta";

/// VirtioFS tag of the Rosetta share (`speck_vz::virtiofs::ROSETTA_TAG`).
pub const ROSETTA_TAG: &str = "rosetta";

/// binfmt_misc rule (Apple's documented magic and mask for x86_64 ELF).
/// Flags: C runs setuid binaries with their own credentials; F opens the
/// interpreter now, so it works inside containers with their own root.
pub const ROSETTA_RULE: &str = concat!(
    ":rosetta:M::",
    r"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x3e\x00",
    ":",
    r"\xff\xff\xff\xff\xff\xfe\xfe\x00\xff\xff\xff\xff\xff\xff\xff\xff\xfe\xff\xff\xff",
    ":/run/rosetta/rosetta:CF",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_has_matching_magic_and_mask() {
        let fields: Vec<&str> = ROSETTA_RULE.split(':').collect();
        // "", name, type, offset, magic, mask, interpreter, flags
        assert_eq!(fields.len(), 8);
        assert_eq!(fields[1], "rosetta");
        assert_eq!(fields[2], "M");
        // Each `\xHH` escape is one byte; other characters are literal.
        let bytes = |s: &str| s.len() - 3 * s.matches(r"\x").count();
        assert_eq!(bytes(fields[4]), 20);
        assert_eq!(bytes(fields[5]), 20);
        assert_eq!(fields[6], format!("{ROSETTA_DIR}/rosetta"));
        assert_eq!(fields[7], "CF");
    }
}
