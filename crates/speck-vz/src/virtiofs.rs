use std::path::Path;

use objc2::AnyThread;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_foundation::{NSArray, NSString, NSURL};
#[cfg(target_os = "macos")]
use objc2_virtualization::{
    VZDirectoryShare, VZDirectorySharingDeviceConfiguration, VZSharedDirectory,
    VZSingleDirectoryShare, VZVirtioFileSystemDeviceConfiguration,
};

use crate::config::VolumeMountConfig;

const SPECK_HOME_TAG: &str = "speck-home";

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
    tag.bytes().all(|b| b.is_ascii_graphic() || b == b'-' || b == b'_')
}

/// Generate the kernel cmdline fragment for VirtioFS mounts.
///
/// Returns a string like:
/// `speck_vol_tags=speck-vol-0:/app,speck-vol-1:/data speck_home_tag=speck-home speck_home_path=/path`
/// The `speck_home` entry is always appended for Ryuk Docker socket access.
pub fn cmdline_virtiofs_arg(mounts: &[VolumeMountConfig], speck_home: &Path) -> String {
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
        " {SPECK_HOME_TAG}_tag={SPECK_HOME_TAG} {SPECK_HOME_TAG}_path={}",
        speck_home.display()
    ));

    result
}

/// Add VirtioFS directory sharing devices to the VM configuration.
///
/// Creates one `VZVirtioFileSystemDeviceConfiguration` per mount, plus
/// an additional device for the `speck-home` tag (Ryuk socket access).
/// All devices are set via `setDirectorySharingDevices` on `vm_config`.
#[cfg(target_os = "macos")]
pub fn configure_virtiofs_devices(
    vm_config: &objc2_virtualization::VZVirtualMachineConfiguration,
    mounts: &[VolumeMountConfig],
    speck_home: &Path,
) -> Result<(), crate::error::Error> {
    let mut fs_devices: Vec<Retained<VZVirtioFileSystemDeviceConfiguration>> =
        Vec::with_capacity(mounts.len() + 1);

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
            VZSingleDirectoryShare::initWithDirectory(
                VZSingleDirectoryShare::alloc(),
                &shared_dir,
            )
        };

        let tag_ns = NSString::from_str(&tag);
        let fs_dev = unsafe {
            VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &tag_ns,
            )
        };
        unsafe { fs_dev.setShare(Some(&share)); }
        fs_devices.push(fs_dev);
    }

    // Add the speck-home device for Ryuk access
    {
        let speck_home_str = NSString::from_str(
            speck_home.to_str().ok_or_else(|| {
                crate::error::Error::VirtioFsMount("non-UTF-8 speck_home path".into())
            })?,
        );
        let speck_home_url = NSURL::fileURLWithPath(&speck_home_str);

        let shared_dir = unsafe {
            VZSharedDirectory::initWithURL_readOnly(
                VZSharedDirectory::alloc(),
                &speck_home_url,
                false,
            )
        };
        let share = unsafe {
            VZSingleDirectoryShare::initWithDirectory(
                VZSingleDirectoryShare::alloc(),
                &shared_dir,
            )
        };

        let tag_ns = NSString::from_str(SPECK_HOME_TAG);
        let fs_dev = unsafe {
            VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &tag_ns,
            )
        };
        unsafe { fs_dev.setShare(Some(&share)); }
        fs_devices.push(fs_dev);
    }

    // Build reference slice for NSArray (coerces via Deref)
    let refs: Vec<&VZDirectorySharingDeviceConfiguration> =
        fs_devices.iter().map(|d| d as &VZDirectorySharingDeviceConfiguration).collect();

    unsafe {
        vm_config.setDirectorySharingDevices(&NSArray::from_slice(&refs));
    }

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
        let result = cmdline_virtiofs_arg(&mounts, home);
        assert!(result.contains("speck_vol_tags=speck-vol-0:/app"));
        assert!(result.contains("speck-home_tag=speck-home"));
        assert!(result.contains("speck-home_path=/tmp/speck-home"));
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
        let result = cmdline_virtiofs_arg(&mounts, home);
        assert!(result.contains("speck_vol_tags=speck-vol-0:/app,speck-vol-1:/etc/config"));
        assert!(result.contains("speck-home_tag=speck-home"));
        assert!(result.contains("speck-home_path=/tmp/speck-home"));
    }

    #[test]
    fn test_cmdline_arg_no_mounts() {
        let mounts: Vec<VolumeMountConfig> = vec![];
        let home = Path::new("/tmp/speck-home");
        let result = cmdline_virtiofs_arg(&mounts, home);
        assert!(!result.contains("speck_vol_tags"));
        assert!(result.contains("speck-home_tag=speck-home"));
    }
}
