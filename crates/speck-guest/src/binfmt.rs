//! binfmt_misc in the arm64 guest: Rosetta for linux/amd64 binaries, and a
//! guard against emulator registrations that take over native binaries.
//!
//! The host shares Apple's Rosetta for Linux over VirtioFS (tag `rosetta`);
//! vminitd mounts it at [`ROSETTA_DIR`] and registers it in binfmt_misc for
//! x86_64 ELF executables. Other architectures (arm/v7, riscv64...) come from
//! `tonistiigi/binfmt --install <arch>`.

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

/// Where the kernel exposes binfmt_misc registrations.
pub const BINFMT_DIR: &str = "/proc/sys/fs/binfmt_misc";

/// One `M` (magic) registration as `/proc/sys/fs/binfmt_misc/<name>` shows it.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub enabled: bool,
    pub interpreter: String,
    pub offset: usize,
    pub magic: Vec<u8>,
    pub mask: Vec<u8>,
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Parse an entry file. `None` for extension (`E`) rules and unreadable text.
pub fn parse_entry(text: &str) -> Option<Entry> {
    let mut entry = Entry {
        enabled: false,
        interpreter: String::new(),
        offset: 0,
        magic: Vec::new(),
        mask: Vec::new(),
    };
    for line in text.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "enabled" => entry.enabled = true,
            "interpreter" => entry.interpreter = value.to_owned(),
            "offset" => entry.offset = value.parse().ok()?,
            "magic" => entry.magic = hex_bytes(value)?,
            "mask" => entry.mask = hex_bytes(value)?,
            _ => {}
        }
    }
    (!entry.magic.is_empty()).then_some(entry)
}

/// Start of a native (little-endian ELF64, EM_AARCH64) executable and
/// shared object: the binaries every arm64 container and the guest run.
fn native_elf_headers() -> [[u8; 20]; 2] {
    let header = |e_type: u8| {
        let mut h = [0u8; 20];
        h[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        h[16] = e_type;
        h[18] = 0xb7;
        h
    };
    [header(2), header(3)]
}

/// Whether an enabled entry would send native arm64 binaries to an
/// emulator. `multiarch/qemu-user-static` is amd64-only; under Rosetta it
/// sees an x86_64 host and registers qemu-aarch64, after which nothing in
/// the guest can start (containerd shims fail with `exec format error`).
pub fn hijacks_native(entry: &Entry) -> bool {
    entry.enabled
        && native_elf_headers().iter().any(|header| {
            entry.magic.iter().enumerate().all(|(i, &magic)| {
                let mask = entry.mask.get(i).copied().unwrap_or(0xff);
                header
                    .get(entry.offset + i)
                    .is_some_and(|&byte| byte & mask == magic & mask)
            })
        })
}

/// Remove every registration in `dir` that hijacks native binaries. Returns
/// `(name, interpreter)` for each one removed. Only reads and writes files,
/// so it works while no arm64 program can start.
pub fn remove_native_hijacks(dir: &std::path::Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for file in entries.flatten() {
        let name = file.file_name().to_string_lossy().into_owned();
        if name == "register" || name == "status" {
            continue;
        }
        let Some(entry) = std::fs::read_to_string(file.path())
            .ok()
            .as_deref()
            .and_then(parse_entry)
        else {
            continue;
        };
        // Writing -1 to an entry file removes the registration.
        if hijacks_native(&entry) && std::fs::write(file.path(), "-1").is_ok() {
            removed.push((name, entry.interpreter));
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cat /proc/sys/fs/binfmt_misc/qemu-aarch64` after
    /// `multiarch/qemu-user-static --reset -p yes` ran under Rosetta.
    const QEMU_AARCH64: &str = "enabled
interpreter /usr/bin/qemu-aarch64-static
flags: F
offset 0
magic 7f454c460201010000000000000000000200b700
mask ffffffffffffff00fffffffffffffffffeffffff
";
    const QEMU_ARM: &str = "enabled
interpreter /usr/bin/qemu-arm
flags: POCF
offset 0
magic 7f454c4601010100000000000000000002002800
mask ffffffffffffff00fffffffffffffffffeffffff
";
    const ROSETTA: &str = "enabled
interpreter /run/rosetta/rosetta
flags: OCF
offset 0
magic 7f454c4602010100000000000000000002003e00
mask fffffffffffefe00fffffffffffffffffeffffff
";

    #[test]
    fn qemu_aarch64_hijacks_native_binaries() {
        let entry = parse_entry(QEMU_AARCH64).unwrap();
        assert_eq!(entry.interpreter, "/usr/bin/qemu-aarch64-static");
        assert_eq!(entry.magic.len(), 20);
        assert!(hijacks_native(&entry));

        let disabled = QEMU_AARCH64.replace("enabled", "disabled");
        assert!(!hijacks_native(&parse_entry(&disabled).unwrap()));
    }

    #[test]
    fn foreign_emulators_are_kept() {
        for text in [QEMU_ARM, ROSETTA] {
            assert!(!hijacks_native(&parse_entry(text).unwrap()), "{text}");
        }
        assert!(parse_entry("enabled\ninterpreter /x\nflags: \nextension .exe\n").is_none());
    }

    #[test]
    fn only_hijacking_entries_are_removed() {
        let dir = std::env::temp_dir().join(format!("speck-binfmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in [
            ("qemu-aarch64", QEMU_AARCH64),
            ("qemu-arm", QEMU_ARM),
            ("rosetta", ROSETTA),
            ("status", "enabled\n"),
        ] {
            std::fs::write(dir.join(name), text).unwrap();
        }

        let removed = remove_native_hijacks(&dir);
        assert_eq!(
            removed,
            [(
                "qemu-aarch64".to_owned(),
                "/usr/bin/qemu-aarch64-static".to_owned()
            )]
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("qemu-aarch64")).unwrap(),
            "-1"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("qemu-arm")).unwrap(),
            QEMU_ARM
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

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
