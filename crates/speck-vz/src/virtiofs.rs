use std::path::{Path, PathBuf};

use objc2::AnyThread;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2_foundation::{NSCopying, NSDictionary, NSMutableDictionary, NSArray, NSString, NSURL};
#[cfg(target_os = "macos")]
use objc2_virtualization::{
    VZDirectoryShare, VZDirectorySharingDeviceConfiguration, VZMultipleDirectoryShare,
    VZSharedDirectory, VZSingleDirectoryShare, VZVirtioFileSystemDevice,
    VZVirtioFileSystemDeviceConfiguration,
};

use crate::config::VolumeMountConfig;

const SPECK_HOME_TAG: &str = "speck-home";
const IDENTITY_TAG_PREFIX: &str = "speck-id-";

/// VirtioFS device tag for the pre-provisioned Docker bind-mount device.
///
/// Speck provisions one `VZVirtioFileSystemDeviceConfiguration` with this tag at
/// VM start time (initially with an empty `VZMultipleDirectoryShare`).  At
/// container start, `update_virtiofs_bind_mounts` replaces the share with one
/// that exposes the requested host paths.  Apple's Virtualization.framework
/// supports updating `VZVirtioFileSystemDevice.share` on a running VM, but does
/// NOT allow adding new devices after the VM starts — hence the pre-provision.
pub const BIND_MOUNTS_TAG: &str = "virtiofs-binds";

/// Convert a container-path to a safe `VZMultipleDirectoryShare` directory name.
///
/// VirtioFS share names appear as directory entries in the guest, so they must
/// not contain `/`.  We replace each `/` with `..` (two dots), which is a valid
/// filesystem character sequence and is guaranteed not to collide with names that
/// contain a single `_` separator:
///
///   `/app`       → `..app`
///   `/etc/conf`  → `..etc..conf`
///   `/a_b`       → `..a_b`   (different from `..a..b` = `/a/b` ✓)
///
/// vminitd reconstructs the original path by reversing the substitution.
pub fn container_path_to_share_name(path: &Path) -> String {
    path.to_string_lossy().replace('/', "..")
}

/// Derive a stable VirtioFS tag for an identity mount path.
///
/// The tag is `speck-id-<basename>` where `<basename>` is the last path
/// component lowercased.  Returns `None` if the path has no file name or
/// the resulting tag exceeds the 36-byte Virtualization.framework limit.
///
/// Examples:
/// - `/Users`      → `Some("speck-id-users")`
/// - `/Volumes`    → `Some("speck-id-volumes")`
/// - `/private/tmp`→ `Some("speck-id-tmp")`
pub fn identity_tag_for_path(path: &Path) -> Option<String> {
    let basename = path.file_name()?.to_str()?.to_ascii_lowercase();
    let tag = format!("{IDENTITY_TAG_PREFIX}{basename}");
    if validate_virtiofs_tag(&tag) { Some(tag) } else { None }
}

/// Validate that a VirtioFS tag meets Virtualization.framework constraints:
/// - Maximum 36 bytes
/// - ASCII only (no nulls, no control characters)
pub fn validate_virtiofs_tag(tag: &str) -> bool {
    if tag.is_empty() {
        return false;
    }
    if tag.len() > 36 {
        return false;
    }
    tag.bytes()
        .all(|b| b.is_ascii_graphic() || b == b'-' || b == b'_')
}

/// Generate the kernel cmdline fragment for VirtioFS mounts.
///
/// Returns a string like:
/// `speck_vol_tags=speck-vol-0:/app speck_home_tag=speck-home speck_home_path=/path speck_identity_tags=speck-id-users:/Users`
///
/// The `speck_home` entry is always appended for Ryuk Docker socket access.
/// Identity mounts (`identity_roots`) are appended as `speck_identity_tags=tag:path,...`
/// when non-empty; paths without a valid tag are skipped.
pub fn cmdline_virtiofs_arg(
    mounts: &[VolumeMountConfig],
    speck_home: &Path,
    identity_roots: &[PathBuf],
) -> String {
    let vol_pairs: Vec<String> = mounts
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let tag = format!("speck-vol-{i}");
            let path = m.container_path.display();
            format!("{tag}:{path}")
        })
        .collect();

    let mut result = if vol_pairs.is_empty() {
        String::new()
    } else {
        format!("speck_vol_tags={}", vol_pairs.join(","))
    };

    result.push_str(&format!(
        " speck_home_tag={SPECK_HOME_TAG} speck_home_path={}",
        speck_home.display()
    ));

    // Append identity mount tags when configured.
    let id_pairs: Vec<String> = identity_roots
        .iter()
        .filter_map(|p| {
            let tag = identity_tag_for_path(p)?;
            let path = p.display();
            Some(format!("{tag}:{path}"))
        })
        .collect();
    if !id_pairs.is_empty() {
        result.push_str(&format!(" speck_identity_tags={}", id_pairs.join(",")));
    }

    result
}

/// Add VirtioFS directory sharing devices to the VM configuration.
///
/// Creates one `VZVirtioFileSystemDeviceConfiguration` per volume mount, plus
/// an additional device for the `speck-home` tag (Ryuk socket access), plus
/// one device per identity mount root (e.g. `/Users`, `/Volumes`).
/// All devices are set via `setDirectorySharingDevices` on `vm_config`.
#[cfg(target_os = "macos")]
pub fn configure_virtiofs_devices(
    vm_config: &objc2_virtualization::VZVirtualMachineConfiguration,
    mounts: &[VolumeMountConfig],
    speck_home: &Path,
    identity_roots: &[PathBuf],
) -> Result<(), crate::error::Error> {
    let mut fs_devices: Vec<Retained<VZVirtioFileSystemDeviceConfiguration>> =
        Vec::with_capacity(mounts.len() + 1 + identity_roots.len());

    for (i, mount) in mounts.iter().enumerate() {
        let tag = format!("speck-vol-{i}");
        if !validate_virtiofs_tag(&tag) {
            return Err(crate::error::Error::VirtioFsMount(format!(
                "invalid VirtioFS tag: {tag}"
            )));
        }

        let host_path_str = NSString::from_str(
            mount
                .host_path
                .to_str()
                .ok_or_else(|| crate::error::Error::VirtioFsMount("non-UTF-8 host path".into()))?,
        );
        let host_url = NSURL::fileURLWithPath(&host_path_str);

        let shared_dir = unsafe {
            VZSharedDirectory::initWithURL_readOnly(
                VZSharedDirectory::alloc(),
                &host_url,
                mount.read_only,
            )
        };
        let share = unsafe {
            VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &shared_dir)
        };

        let tag_ns = NSString::from_str(&tag);
        let fs_dev = unsafe {
            VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &tag_ns,
            )
        };
        unsafe {
            fs_dev.setShare(Some(&share));
        }
        fs_devices.push(fs_dev);
    }

    // Add the speck-home device for Ryuk access
    {
        let speck_home_str = NSString::from_str(speck_home.to_str().ok_or_else(|| {
            crate::error::Error::VirtioFsMount("non-UTF-8 speck_home path".into())
        })?);
        let speck_home_url = NSURL::fileURLWithPath(&speck_home_str);

        let shared_dir = unsafe {
            VZSharedDirectory::initWithURL_readOnly(
                VZSharedDirectory::alloc(),
                &speck_home_url,
                false,
            )
        };
        let share = unsafe {
            VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &shared_dir)
        };

        let tag_ns = NSString::from_str(SPECK_HOME_TAG);
        let fs_dev = unsafe {
            VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &tag_ns,
            )
        };
        unsafe {
            fs_dev.setShare(Some(&share));
        }
        fs_devices.push(fs_dev);
    }

    // Add identity mount devices (e.g. /Users, /Volumes, /private/tmp).
    // Paths that do not exist or have no valid tag are skipped.
    for host_path in identity_roots {
        if !host_path.exists() {
            continue;
        }
        let tag = match identity_tag_for_path(host_path) {
            Some(t) => t,
            None => continue,
        };

        let path_str = match host_path.to_str() {
            Some(s) => NSString::from_str(s),
            None => continue,
        };
        let url = NSURL::fileURLWithPath(&path_str);

        let shared_dir = unsafe {
            VZSharedDirectory::initWithURL_readOnly(VZSharedDirectory::alloc(), &url, true)
        };
        let share = unsafe {
            VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &shared_dir)
        };

        let tag_ns = NSString::from_str(&tag);
        let fs_dev = unsafe {
            VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &tag_ns,
            )
        };
        unsafe {
            fs_dev.setShare(Some(&share));
        }
        fs_devices.push(fs_dev);
    }

    // ── Pre-provisioned bind-mount device (D-05) ───────────────────────
    // One VZVirtioFileSystemDeviceConfiguration with BIND_MOUNTS_TAG is always
    // added to the VM configuration.  It starts with an empty
    // VZMultipleDirectoryShare and is updated at container start time via
    // `update_virtiofs_bind_mounts`.  New VirtioFS devices cannot be hot-added
    // to a running VM, so this slot must be reserved at configuration time.
    {
        let bind_tag = NSString::from_str(BIND_MOUNTS_TAG);
        let empty_share = unsafe {
            VZMultipleDirectoryShare::init(VZMultipleDirectoryShare::alloc())
        };
        let bind_dev = unsafe {
            VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &bind_tag,
            )
        };
        let share_ref: &VZDirectoryShare = &empty_share;
        unsafe { bind_dev.setShare(Some(share_ref)) };
        fs_devices.push(bind_dev);
    }

    // Build reference slice for NSArray (coerces via Deref)
    let refs: Vec<&VZDirectorySharingDeviceConfiguration> = fs_devices
        .iter()
        .map(|d| d as &VZDirectorySharingDeviceConfiguration)
        .collect();

    unsafe {
        vm_config.setDirectorySharingDevices(&NSArray::from_slice(&refs));
    }

    Ok(())
}

/// Update the pre-provisioned `virtiofs-binds` device on a running VM.
///
/// Finds the [`BIND_MOUNTS_TAG`] device in `vm.directorySharingDevices()`,
/// builds a new [`VZMultipleDirectoryShare`] from `binds`, and swaps the share.
/// Apple confirms this is the supported runtime-update path — new devices cannot
/// be added after VM start but an existing device's `share` is mutable.
///
/// Returns `Err(VirtioFsMount)` if the pre-provisioned device is not found (e.g.
/// the VM was started without `configure_virtiofs_devices`).
#[cfg(target_os = "macos")]
pub fn update_virtiofs_bind_mounts(
    vm: &objc2_virtualization::VZVirtualMachine,
    binds: &[VolumeMountConfig],
) -> Result<(), crate::error::Error> {
    // ── Find the virtiofs-binds device ─────────────────────────────────
    let devices = unsafe { vm.directorySharingDevices() };
    let mut bind_device: Option<Retained<VZVirtioFileSystemDevice>> = None;
    let count = devices.count();
    for i in 0..count {
        let device = unsafe { devices.objectAtIndex(i) };
        if let Ok(fs_dev) = device.downcast::<VZVirtioFileSystemDevice>() {
            let tag = unsafe { fs_dev.tag() };
            let is_match = objc2::rc::autoreleasepool(|pool| {
                let tag_str = unsafe { tag.to_str(pool) };
                tag_str == BIND_MOUNTS_TAG
            });
            if is_match {
                bind_device = Some(fs_dev);
                break;
            }
        }
    }

    let device = bind_device.ok_or_else(|| {
        crate::error::Error::VirtioFsMount(
            "virtiofs-binds device not found in running VM; \
             was configure_virtiofs_devices called?"
                .into(),
        )
    })?;

    // ── Build the new VZMultipleDirectoryShare ──────────────────────────
    // NSMutableDictionary<NSString, VZSharedDirectory> is built entry by entry,
    // then passed to VZMultipleDirectoryShare::initWithDirectories.
    let mut_dict =
        NSMutableDictionary::<NSString, VZSharedDirectory>::init(NSMutableDictionary::alloc());

    for bind in binds {
        let share_name = container_path_to_share_name(&bind.container_path);

        // Validate the share name using Apple's own canonicalization.
        let name_ns = NSString::from_str(&share_name);
        let canonical = unsafe { VZMultipleDirectoryShare::canonicalizedNameFromName(&name_ns) };
        let canonical = match canonical {
            Some(c) => c,
            None => {
                tracing::warn!(
                    container_path = %bind.container_path.display(),
                    share_name = %share_name,
                    "VZMultipleDirectoryShare rejected share name; skipping bind mount"
                );
                continue;
            }
        };

        let host_str = NSString::from_str(
            bind.host_path.to_str().ok_or_else(|| {
                crate::error::Error::VirtioFsMount("non-UTF-8 bind host path".into())
            })?,
        );
        let host_url = NSURL::fileURLWithPath(&host_str);
        let shared_dir = unsafe {
            VZSharedDirectory::initWithURL_readOnly(
                VZSharedDirectory::alloc(),
                &host_url,
                bind.read_only,
            )
        };

        // NSMutableDictionary.setObject_forKey requires the key to be NSCopying.
        let key_copying: &ProtocolObject<dyn NSCopying> =
            ProtocolObject::from_ref(&*canonical);
        unsafe { mut_dict.setObject_forKey(&*shared_dir, key_copying) };
    }

    // Cast NSMutableDictionary → &NSDictionary via Deref coercion.
    let dict_ref: &NSDictionary<NSString, VZSharedDirectory> = &*mut_dict;
    let new_share = unsafe {
        VZMultipleDirectoryShare::initWithDirectories(VZMultipleDirectoryShare::alloc(), dict_ref)
    };

    let share_ref: &VZDirectoryShare = &new_share;
    unsafe { device.setShare(Some(share_ref)) };

    tracing::debug!(
        bind_count = binds.len(),
        "updated virtiofs-binds VZMultipleDirectoryShare on running VM"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tag_valid_alphanumeric() {
        assert!(validate_virtiofs_tag("speck-vol-0"));
        assert!(validate_virtiofs_tag("speck-home"));
        assert!(validate_virtiofs_tag("a"));
    }

    #[test]
    fn test_tag_empty_rejected() {
        assert!(!validate_virtiofs_tag(""));
    }

    #[test]
    fn test_tag_too_long_36_bytes() {
        let tag_ok = "a".repeat(36);
        assert!(validate_virtiofs_tag(&tag_ok));
        let tag_too_long = "a".repeat(37);
        assert!(!validate_virtiofs_tag(&tag_too_long));
    }

    #[test]
    fn test_tag_non_ascii_rejected() {
        assert!(!validate_virtiofs_tag("speck-vol-\u{00e9}"));
        assert!(!validate_virtiofs_tag("tag\x00null"));
    }

    #[test]
    fn test_cmdline_arg_single_mount() {
        let mounts = vec![VolumeMountConfig {
            host_path: "/Users/user/data".into(),
            container_path: "/app".into(),
            read_only: false,
            volume_name: None,
        }];
        let home = Path::new("/tmp/speck-home");
        let result = cmdline_virtiofs_arg(&mounts, home, &[]);
        assert!(result.contains("speck_vol_tags=speck-vol-0:/app"));
        assert!(result.contains("speck_home_tag=speck-home"));
        assert!(result.contains("speck_home_path=/tmp/speck-home"));
    }

    #[test]
    fn test_cmdline_arg_two_mounts() {
        let mounts = vec![
            VolumeMountConfig {
                host_path: "/Users/user/data".into(),
                container_path: "/app".into(),
                read_only: false,
                volume_name: None,
            },
            VolumeMountConfig {
                host_path: "/Users/user/config".into(),
                container_path: "/etc/config".into(),
                read_only: true,
                volume_name: None,
            },
        ];
        let home = Path::new("/tmp/speck-home");
        let result = cmdline_virtiofs_arg(&mounts, home, &[]);
        assert!(result.contains("speck_vol_tags=speck-vol-0:/app,speck-vol-1:/etc/config"));
        assert!(result.contains("speck_home_tag=speck-home"));
        assert!(result.contains("speck_home_path=/tmp/speck-home"));
    }

    #[test]
    fn test_cmdline_arg_no_mounts() {
        let mounts: Vec<VolumeMountConfig> = vec![];
        let home = Path::new("/tmp/speck-home");
        let result = cmdline_virtiofs_arg(&mounts, home, &[]);
        assert!(!result.contains("speck_vol_tags"));
        assert!(result.contains("speck_home_tag=speck-home"));
    }

    #[test]
    fn test_identity_tag_for_known_paths() {
        assert_eq!(
            identity_tag_for_path(Path::new("/Users")),
            Some("speck-id-users".to_string())
        );
        assert_eq!(
            identity_tag_for_path(Path::new("/Volumes")),
            Some("speck-id-volumes".to_string())
        );
        assert_eq!(
            identity_tag_for_path(Path::new("/private/tmp")),
            Some("speck-id-tmp".to_string())
        );
        // Root path has no file_name component
        assert_eq!(identity_tag_for_path(Path::new("/")), None);
    }

    #[test]
    fn test_cmdline_identity_tags_included() {
        let mounts: Vec<VolumeMountConfig> = vec![];
        let home = Path::new("/tmp/speck-home");
        let identity_roots = vec![
            PathBuf::from("/Users"),
            PathBuf::from("/Volumes"),
        ];
        let result = cmdline_virtiofs_arg(&mounts, home, &identity_roots);
        assert!(
            result.contains("speck_identity_tags=speck-id-users:/Users,speck-id-volumes:/Volumes"),
            "identity tags missing from cmdline: {result}"
        );
    }

    #[test]
    fn test_cmdline_no_identity_tags_when_empty() {
        let mounts: Vec<VolumeMountConfig> = vec![];
        let home = Path::new("/tmp/speck-home");
        let result = cmdline_virtiofs_arg(&mounts, home, &[]);
        assert!(
            !result.contains("speck_identity_tags"),
            "unexpected identity_tags in cmdline: {result}"
        );
    }

    // ── D-05: pre-provisioned bind-mounts device ────────────────────────

    #[test]
    fn test_bind_mounts_tag_is_valid_virtiofs_tag() {
        // BIND_MOUNTS_TAG must be a valid VirtioFS tag so the pre-provisioned
        // virtiofs-binds device passes Apple's configuration validation.
        assert!(
            validate_virtiofs_tag(BIND_MOUNTS_TAG),
            "BIND_MOUNTS_TAG must be a valid VirtioFS tag; got: {BIND_MOUNTS_TAG}"
        );
    }

    #[test]
    fn test_container_path_to_share_name_simple() {
        let name = container_path_to_share_name(std::path::Path::new("/app"));
        assert!(!name.contains('/'), "share name must not contain slashes; got: {name}");
        assert!(!name.is_empty(), "share name must not be empty");
        // Leading slash → leading separator
        assert_eq!(name, "..app", "expected separator-prefixed name");
    }

    #[test]
    fn test_container_path_to_share_name_nested() {
        let name = container_path_to_share_name(std::path::Path::new("/etc/config"));
        assert!(!name.contains('/'), "nested path must have slashes replaced");
        assert_eq!(name, "..etc..config", "expected double-dot separator");
    }

    #[test]
    fn test_container_path_to_share_name_no_collision() {
        // /a_b and /a/b must produce different names when using the ../ scheme.
        let a = container_path_to_share_name(std::path::Path::new("/a_b"));
        let b = container_path_to_share_name(std::path::Path::new("/a/b"));
        assert_ne!(a, b, "collision: /a_b and /a/b must not map to the same share name");
    }
}
