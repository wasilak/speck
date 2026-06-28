//! vminitd — PID 1 inside the speck micro-VM.
//!
//! Parses the kernel cmdline for port and disk configuration, mounts early
//! filesystems (/proc, /sys, /dev), mounts the rootfs and data disks,
//! spawns and supervises containerd + buildkitd with restart loops,
//! health-checks containerd readiness, sends a READY signal to the host
//! over vsock, and starts vsock→Unix socket forwarders for containerd
//! and buildkitd gRPC access.
//!
//! Built as a static musl binary, placed at /init in a cpio initrd.
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("vminitd is only runnable inside the Linux guest");
}

#[cfg(target_os = "linux")]
fn main() {
    linux::main();
}

#[cfg(target_os = "linux")]
mod linux {

    use std::io;

    pub(super) fn main() {
        let _port = parse_cmdline_vsock_port("/proc/cmdline").unwrap_or(1234);
        let dns_port = parse_cmdline_dns_port("/proc/cmdline");
        let containerd_port = parse_cmdline_containerd_vsock_port("/proc/cmdline").unwrap_or(9001);
        let buildkitd_port = parse_cmdline_buildkitd_vsock_port("/proc/cmdline").unwrap_or(9002);
        let ready_port = parse_cmdline_ready_vsock_port("/proc/cmdline").unwrap_or(9000);

        // Mount filesystems and disks before spawning services
        mount_early_filesystems();
        mount_disks();

        // Mount VirtioFS volumes and Speck home for Ryuk socket access
        mount_virtiofs_volumes("/proc/cmdline");
        mount_speck_home("/proc/cmdline");
        let vol_tags = parse_cmdline_volume_tags("/proc/cmdline");
        create_named_volume_dirs(&vol_tags);
        eprintln!("vminitd: mounted {} virtiofs devices", vol_tags.len());

        // If DNS proxy vsock port is configured, spawn the DNS forwarder
        if let Some(dns_vsock_port) = dns_port {
            std::thread::spawn(
                move || match speck_guest::dns_forwarder::serve(dns_vsock_port) {
                    Ok(()) => eprintln!("vminitd: dns forwarder finished (guest shutdown)"),
                    Err(e) => eprintln!("vminitd: dns forwarder error: {e}"),
                },
            );
        }

        // Spawn and supervise containerd and buildkitd
        spawn_service_with_restart(
            "containerd",
            "/rootfs/usr/bin/containerd",
            vec!["--config", "/rootfs/etc/containerd/config.toml"],
        );
        spawn_service_with_restart("buildkitd", "/rootfs/usr/local/bin/buildkitd", vec![]);

        // Wait for containerd socket to become reachable
        if !wait_for_containerd_socket(50) {
            eprintln!("vminitd: containerd did not become ready within timeout");
            std::process::exit(1);
        }

        // Signal READY to the host (D-05)
        send_ready_signal(ready_port);

        // Start vsock→Unix socket forwarders (D-07, D-09)
        std::thread::spawn(move || {
            if let Err(e) = speck_guest::sock_forwarder::serve(
                containerd_port,
                "/rootfs/run/containerd/containerd.sock",
            ) {
                eprintln!("vminitd: containerd forwarder error: {e}");
            }
        });
        std::thread::spawn(move || {
            if let Err(e) = speck_guest::sock_forwarder::serve(
                buildkitd_port,
                "/rootfs/run/buildkit/buildkitd.sock",
            ) {
                eprintln!("vminitd: buildkitd forwarder error: {e}");
            }
        });

        // PID 1 must never exit
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    // ---------------------------------------------------------------------------
    // Kernel cmdline parsing
    // ---------------------------------------------------------------------------

    /// Parse `vsock_port=PORT` from the kernel command line.
    fn parse_cmdline_vsock_port(path: &str) -> Option<u32> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(port_str) = word.strip_prefix("vsock_port=") {
                return port_str.parse::<u32>().ok();
            }
        }
        None
    }

    /// Parse `dns_vsock_port=PORT` from the kernel command line.
    fn parse_cmdline_dns_port(path: &str) -> Option<u32> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(port_str) = word.strip_prefix("dns_vsock_port=") {
                return port_str.parse::<u32>().ok();
            }
        }
        None
    }

    /// Parse `containerd_vsock_port=PORT` from the kernel command line.
    fn parse_cmdline_containerd_vsock_port(path: &str) -> Option<u32> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(port_str) = word.strip_prefix("containerd_vsock_port=") {
                return port_str.parse::<u32>().ok();
            }
        }
        None
    }

    /// Parse `buildkitd_vsock_port=PORT` from the kernel command line.
    fn parse_cmdline_buildkitd_vsock_port(path: &str) -> Option<u32> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(port_str) = word.strip_prefix("buildkitd_vsock_port=") {
                return port_str.parse::<u32>().ok();
            }
        }
        None
    }

    /// Parse `ready_vsock_port=PORT` from the kernel command line.
    fn parse_cmdline_ready_vsock_port(path: &str) -> Option<u32> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(port_str) = word.strip_prefix("ready_vsock_port=") {
                return port_str.parse::<u32>().ok();
            }
        }
        None
    }

    // ---------------------------------------------------------------------------
    // Filesystem mounting
    // ---------------------------------------------------------------------------

    /// Mount /proc, /sys, and /dev (devtmpfs) before any disk operations.
    ///
    /// Each mount is non-fatal — errors are logged but execution continues
    /// (may already be mounted by the kernel during early boot).
    fn mount_early_filesystems() {
        // procfs
        let ret = unsafe {
            libc::mount(
                b"proc\0".as_ptr() as *const libc::c_char,
                b"/proc\0".as_ptr() as *const libc::c_char,
                b"proc\0".as_ptr() as *const libc::c_char,
                0,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: mount /proc failed: {:?}",
                io::Error::last_os_error()
            );
        }

        // sysfs
        let ret = unsafe {
            libc::mount(
                b"sysfs\0".as_ptr() as *const libc::c_char,
                b"/sys\0".as_ptr() as *const libc::c_char,
                b"sysfs\0".as_ptr() as *const libc::c_char,
                0,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: mount /sys failed: {:?}",
                io::Error::last_os_error()
            );
        }

        // devtmpfs
        let ret = unsafe {
            libc::mount(
                b"devtmpfs\0".as_ptr() as *const libc::c_char,
                b"/dev\0".as_ptr() as *const libc::c_char,
                b"devtmpfs\0".as_ptr() as *const libc::c_char,
                0,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: mount /dev failed: {:?}",
                io::Error::last_os_error()
            );
        }
    }

    /// Mount the rootfs and data disks.
    ///
    /// 1. Mount /dev/vda (rootfs) at /rootfs as ext4.
    /// 2. Mount /dev/vdb (data disk) at /rootfs/var/lib/containerd as ext4.
    ///    If the data disk has no filesystem (ENODEV), format it with mke2fs
    ///    and retry. Failure on the second attempt is fatal.
    /// 3. Create /rootfs/run/ and /rootfs/tmp/.
    fn mount_disks() {
        // Create root mount point
        let _ = std::fs::create_dir_all("/rootfs");

        // Mount /dev/vda → /rootfs (rootfs disk)
        let ret = unsafe {
            libc::mount(
                b"/dev/vda\0".as_ptr() as *const libc::c_char,
                b"/rootfs\0".as_ptr() as *const libc::c_char,
                b"ext4\0".as_ptr() as *const libc::c_char,
                libc::MS_RELATIME,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: mount /dev/vda → /rootfs failed: {:?}",
                io::Error::last_os_error()
            );
            std::process::exit(1);
        }

        // Create containerd data directory on rootfs
        let _ = std::fs::create_dir_all("/rootfs/var/lib/containerd");

        // Try to mount /dev/vdb → /rootfs/var/lib/containerd (data disk)
        let ret = unsafe {
            libc::mount(
                b"/dev/vdb\0".as_ptr() as *const libc::c_char,
                b"/rootfs/var/lib/containerd\0".as_ptr() as *const libc::c_char,
                b"ext4\0".as_ptr() as *const libc::c_char,
                libc::MS_RELATIME,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ENODEV) {
                eprintln!("vminitd: /dev/vdb has no filesystem — formatting with mke2fs");
                unsafe {
                    libc::system(b"mke2fs -t ext4 /dev/vdb\0".as_ptr() as *const libc::c_char);
                }
                // Retry mount after formatting
                let ret = unsafe {
                    libc::mount(
                        b"/dev/vdb\0".as_ptr() as *const libc::c_char,
                        b"/rootfs/var/lib/containerd\0".as_ptr() as *const libc::c_char,
                        b"ext4\0".as_ptr() as *const libc::c_char,
                        libc::MS_RELATIME,
                        std::ptr::null(),
                    )
                };
                if ret < 0 {
                    eprintln!(
                        "vminitd: mount /dev/vdb → /rootfs/var/lib/containerd failed after format: {:?}",
                        io::Error::last_os_error()
                    );
                    std::process::exit(1);
                }
            } else {
                eprintln!(
                    "vminitd: mount /dev/vdb → /rootfs/var/lib/containerd failed: {:?}",
                    err
                );
                std::process::exit(1);
            }
        }

        // Create runtime directories needed by containerd
        let _ = std::fs::create_dir_all("/rootfs/run");
        let _ = std::fs::create_dir_all("/rootfs/tmp");
    }

    // ---------------------------------------------------------------------------
    // containerd health check
    // ---------------------------------------------------------------------------

    /// Wait for containerd's Unix socket to become reachable.
    ///
    /// Polls `/rootfs/run/containerd/containerd.sock` by attempting to connect.
    /// Returns `true` if the socket accept a connection within `max_attempts`
    /// (200ms interval). Returns `false` if all attempts are exhausted.
    fn wait_for_containerd_socket(max_attempts: u32) -> bool {
        for attempt in 0..max_attempts {
            if unix_connect_once("/rootfs/run/containerd/containerd.sock") {
                eprintln!("vminitd: containerd socket ready after {attempt} attempts");
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        false
    }

    /// Attempt an AF_UNIX connect to `path` and immediately close the fd.
    /// Returns `true` if the connection succeeded.
    fn unix_connect_once(path: &str) -> bool {
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return false;
        }

        let path_bytes = path.as_bytes();
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as u16;
        for (i, &b) in path_bytes.iter().enumerate() {
            if i >= 108 {
                break;
            }
            addr.sun_path[i] = b;
        }

        let addr_ptr = &addr as *const libc::sockaddr_un as *const libc::sockaddr;
        let addr_len = std::mem::size_of::<libc::sockaddr_un>() as u32;
        let ret = unsafe { libc::connect(fd, addr_ptr, addr_len) };
        unsafe {
            libc::close(fd);
        }
        ret == 0
    }

    // ---------------------------------------------------------------------------
    // READY signal
    // ---------------------------------------------------------------------------

    /// Send the READY signal to the host over vsock.
    ///
    /// Listens on `ready_vsock_port`, accepts one connection from the host,
    /// and writes `b"READY\n"`. Non-fatal on error (logged via eprintln).
    fn send_ready_signal(ready_vsock_port: u32) {
        let listen_fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
        if listen_fd < 0 {
            eprintln!(
                "vminitd: send_ready_signal socket failed: {:?}",
                io::Error::last_os_error()
            );
            return;
        }

        let addr = libc::sockaddr_vm {
            svm_family: libc::AF_VSOCK as u16,
            svm_reserved1: 0,
            svm_port: ready_vsock_port,
            svm_cid: libc::VMADDR_CID_ANY,
            svm_zero: [0u8; 4],
        };

        let addr_ptr = &addr as *const libc::sockaddr_vm as *const libc::sockaddr;
        let addr_len = std::mem::size_of::<libc::sockaddr_vm>() as u32;

        let ret = unsafe { libc::bind(listen_fd, addr_ptr, addr_len) };
        if ret < 0 {
            eprintln!(
                "vminitd: send_ready_signal bind failed: {:?}",
                io::Error::last_os_error()
            );
            unsafe {
                libc::close(listen_fd);
            }
            return;
        }

        let ret = unsafe { libc::listen(listen_fd, 1) };
        if ret < 0 {
            eprintln!(
                "vminitd: send_ready_signal listen failed: {:?}",
                io::Error::last_os_error()
            );
            unsafe {
                libc::close(listen_fd);
            }
            return;
        }

        let conn_fd =
            unsafe { libc::accept(listen_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if conn_fd < 0 {
            eprintln!(
                "vminitd: send_ready_signal accept failed: {:?}",
                io::Error::last_os_error()
            );
            unsafe {
                libc::close(listen_fd);
            }
            return;
        }

        // Write READY signal
        let msg = b"READY\n";
        let mut written = 0usize;
        while written < msg.len() {
            let n = unsafe {
                libc::write(
                    conn_fd,
                    msg.as_ptr().add(written) as *const libc::c_void,
                    msg.len() - written,
                )
            };
            if n < 0 {
                eprintln!(
                    "vminitd: send_ready_signal write failed: {:?}",
                    io::Error::last_os_error()
                );
                break;
            }
            written += n as usize;
        }

        unsafe {
            libc::close(conn_fd);
        }
        unsafe {
            libc::close(listen_fd);
        }

        if written == msg.len() {
            eprintln!("vminitd: READY signal sent on vsock port {ready_vsock_port}");
        }
    }

    // ---------------------------------------------------------------------------
    // VirtioFS mounting
    // ---------------------------------------------------------------------------

    /// Parse `speck_vol_tags=tag0:path0,tag1:path1` from the kernel cmdline.
    ///
    /// Returns a list of (tag, container_path) pairs that vminitd will mount.
    fn parse_cmdline_volume_tags(path: &str) -> Vec<(String, String)> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        for word in content.split_whitespace() {
            if let Some(val) = word.strip_prefix("speck_vol_tags=") {
                return val
                    .split(',')
                    .filter_map(|pair| {
                        let mut parts = pair.splitn(2, ':');
                        let tag = parts.next()?.to_string();
                        let cpath = parts.next()?.to_string();
                        Some((tag, cpath))
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    /// Parse `speck_home_path=PATH` from the kernel cmdline.
    fn parse_cmdline_speck_home(path: &str) -> Option<String> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(val) = word.strip_prefix("speck_home_path=") {
                // Reject empty paths
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
        None
    }

    /// Parse `speck_home_tag=TAG` from the kernel cmdline.
    fn parse_cmdline_speck_home_tag(path: &str) -> Option<String> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(val) = word.strip_prefix("speck_home_tag=") {
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
        None
    }

    /// Mount VirtioFS volumes for user-specified bind mounts.
    ///
    /// Parses `speck_vol_tags=` from the kernel cmdline and for each
    /// (tag, container_path) pair, creates the target directory under
    /// `/rootfs` and mounts the VirtioFS filesystem.
    ///
    /// Mount failures are non-fatal — a container simply won't see its
    /// bind mount if the tag is invalid.
    fn mount_virtiofs_volumes(cmdline_path: &str) {
        let tags = parse_cmdline_volume_tags(cmdline_path);
        for (tag, container_path) in &tags {
            let target = format!("/rootfs{container_path}");
            let _ = std::fs::create_dir_all(&target);

            let tag_c = std::ffi::CString::new(tag.as_str()).unwrap_or_default();
            let target_c = std::ffi::CString::new(target.as_str()).unwrap_or_default();

            let ret = unsafe {
                libc::mount(
                    tag_c.as_ptr(),
                    target_c.as_ptr(),
                    b"virtiofs\0".as_ptr() as *const libc::c_char,
                    0,
                    std::ptr::null(),
                )
            };
            if ret < 0 {
                eprintln!(
                    "vminitd: failed to mount VirtioFS {tag} at {target}: {:?}",
                    io::Error::last_os_error()
                );
            } else {
                eprintln!("vminitd: mounted VirtioFS {tag} at {target}");
            }
        }
    }

    /// Mount the Speck home directory at `/rootfs/var/run/` and create the
    /// Docker socket symlink for Ryuk.
    ///
    /// The Speck Docker socket is exposed to containers via
    /// `/var/run/docker.sock → /var/run/speck.sock`. If the host has a
    /// `docker.sock` file inside `$SPECK_HOME`, Ryuk and testcontainers
    /// can use it.
    ///
    /// Non-fatal on error — Ryuk will simply not have Docker socket access.
    fn mount_speck_home(cmdline_path: &str) {
        let tag = match parse_cmdline_speck_home_tag(cmdline_path) {
            Some(t) => t,
            None => return,
        };
        let _ = std::fs::create_dir_all("/rootfs/var/run");

        let tag_c = std::ffi::CString::new(tag.as_str()).unwrap_or_default();
        let target_c = std::ffi::CString::new("/rootfs/var/run").unwrap_or_default();

        let ret = unsafe {
            libc::mount(
                tag_c.as_ptr(),
                target_c.as_ptr(),
                b"virtiofs\0".as_ptr() as *const libc::c_char,
                0,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: failed to mount Speck home at /rootfs/var/run: {:?}",
                io::Error::last_os_error()
            );
            return;
        }

        // Create symlink docker.sock → speck.sock for Ryuk
        let ret = unsafe {
            libc::symlink(
                b"/var/run/speck.sock\0".as_ptr() as *const libc::c_char,
                b"/rootfs/var/run/docker.sock\0".as_ptr() as *const libc::c_char,
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: failed to create docker.sock symlink: {:?}",
                io::Error::last_os_error()
            );
        } else {
            eprintln!("vminitd: created /var/run/docker.sock → /var/run/speck.sock");
        }
    }

    /// Create named volume directories under `/rootfs/var/lib/speck/volumes/`.
    ///
    /// These directories back Docker named volumes on the data disk. The mount
    /// function matches container_path entries that live under the volume
    /// store path (e.g., `/var/lib/speck/volumes/myvolume`).
    fn create_named_volume_dirs(tags: &[(String, String)]) {
        for (_tag, container_path) in tags {
            if container_path.starts_with("/var/lib/speck/volumes/") {
                let target = format!("/rootfs{container_path}");
                match std::fs::create_dir_all(&target) {
                    Ok(()) => {
                        eprintln!("vminitd: created named volume directory {target}");
                    }
                    Err(e) => {
                        eprintln!("vminitd: failed to create named volume dir {target}: {e}");
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Service supervision
    // ---------------------------------------------------------------------------

    /// Spawn a background thread that runs `program` with `args` in a restart
    /// loop. On crash, waits 1 second before restarting (backoff D-06).
    fn spawn_service_with_restart(
        name: &'static str,
        program: &'static str,
        args: Vec<&'static str>,
    ) {
        std::thread::spawn(move || {
            loop {
                match std::process::Command::new(program).args(&args).spawn() {
                    Ok(mut child) => {
                        eprintln!("vminitd: started {name} (pid {})", child.id());
                        match child.wait() {
                            Ok(status) => {
                                eprintln!(
                                    "vminitd: {name} exited with status {status} — restarting"
                                );
                            }
                            Err(e) => {
                                eprintln!("vminitd: {name} wait error: {e} — restarting");
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("vminitd: failed to spawn {name} ({program}): {e} — retrying");
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }
}
