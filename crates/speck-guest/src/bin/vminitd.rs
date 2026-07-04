//! vminitd — PID 1 inside the speck micro-VM.
//!
//! Parses the kernel cmdline for port and disk configuration, mounts early
//! filesystems (/proc, /sys, /dev), mounts the rootfs and data disks,
//! spawns dockerd with auto-restart, health-checks the Docker socket,
//! sends a READY signal to the host over vsock, and starts a vsock→Unix
//! socket forwarder for the Docker Engine API.
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
        // Mount early filesystems (/proc, /sys, /dev) before reading
        // /proc/cmdline — the kernel cmdline lives at /proc/cmdline and
        // all cmdline parsers below depend on /proc being reachable.
        mount_early_filesystems();

        let _port = parse_cmdline_vsock_port("/proc/cmdline").unwrap_or(1234);
        let dns_port = parse_cmdline_dns_port("/proc/cmdline");
        let ready_port = parse_cmdline_ready_vsock_port("/proc/cmdline").unwrap_or(9000);
        let docker_port = parse_cmdline_docker_vsock_port("/proc/cmdline").unwrap_or(9003);
        let guest_ip = parse_cmdline_guest_ip("/proc/cmdline");
        let gateway = parse_cmdline_gateway("/proc/cmdline");
        eprintln!("vminitd: docker_port={docker_port}");
        if let Some(ip) = guest_ip.as_deref() {
            eprintln!("vminitd: guest_ip={ip}");
        }
        if let Some(gw) = gateway.as_deref() {
            eprintln!("vminitd: gateway={gw}");
        }

        mount_disks();

        // Mount VirtioFS volumes and Speck home for Ryuk socket access
        mount_virtiofs_volumes("/proc/cmdline");
        mount_speck_home("/proc/cmdline");
        let vol_tags = parse_cmdline_volume_tags("/proc/cmdline");
        create_named_volume_dirs(&vol_tags);
        eprintln!("vminitd: mounted {} virtiofs devices", vol_tags.len());

        // Mount identity roots so Docker bind mount absolute paths resolve inside
        // the guest chroot (e.g. /Users/... is valid inside /rootfs/Users/...).
        mount_identity_roots("/proc/cmdline");

        // If DNS proxy vsock port is configured, spawn the DNS forwarder.
        // DNS forwarding is always active regardless of container backend
        // because the host resolver is the source of truth for VPN/WARP DNS.
        if let Some(dns_vsock_port) = dns_port {
            std::thread::spawn(
                move || match speck_guest::dns_forwarder::serve(dns_vsock_port) {
                    Ok(()) => eprintln!("vminitd: dns forwarder finished (guest shutdown)"),
                    Err(e) => eprintln!("vminitd: dns forwarder error: {e}"),
                },
            );
        }

        // Bring up loopback first so 127.0.0.1 routes locally (kernel adds
        // 127.0.0.0/8 to its local table only when lo is UP).
        bring_up_loopback();

        // Bring up the virtio-net interface so containers can reach external
        // registries.  This is required even though DNS goes through the vsock
        // proxy — image pulls and other TCP/UDP traffic need a live network.
        if let (Some(ip_str), Some(gw_str)) = (guest_ip, gateway) {
            bring_up_network(&ip_str, &gw_str);
        }

        // Write /etc/resolv.conf inside the chroot so dockerd sends DNS queries
        // to the guest-local forwarder (listening on UDP:53) instead of hitting
        // an external resolver that is unreachable without a routed connection.
        if let Err(e) = std::fs::write(
            "/rootfs/etc/resolv.conf",
            "nameserver 127.0.0.1\n",
        ) {
            eprintln!("vminitd: failed to write /rootfs/etc/resolv.conf: {e}");
        } else {
            eprintln!("vminitd: wrote /rootfs/etc/resolv.conf → nameserver 127.0.0.1");
        }

        // Mount proc/sys/dev/run into /rootfs so dockerd can see them.
        mount_rootfs_runtime_filesystems();

        // Create the socket directory inside the chroot.
        if let Err(e) = std::fs::create_dir_all("/rootfs/run/speck") {
            eprintln!("vminitd: failed to create /rootfs/run/speck: {e}");
        }

        let dockerd_bin = detect_dockerd_bin_in_chroot();
        eprintln!("vminitd: using dockerd at chroot-relative path {dockerd_bin}");
        spawn_dockerd_with_restart(dockerd_bin);

        if !wait_for_dockerd_socket(150) {
            eprintln!("vminitd: dockerd socket did not become ready within timeout");
            std::process::exit(1);
        }
        eprintln!("vminitd: dockerd socket ready");

        send_ready_signal(ready_port);

        // Forward dockerd's Unix socket over vsock port 9003.
        std::thread::spawn(move || {
            if let Err(e) = speck_guest::sock_forwarder::serve(
                docker_port,
                "/rootfs/run/speck/dockerd.sock",
            ) {
                eprintln!("vminitd: dockerd forwarder error: {e}");
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

    /// Parse `speck_guest_ip=IP` from the kernel command line.
    fn parse_cmdline_guest_ip(path: &str) -> Option<String> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(val) = word.strip_prefix("speck_guest_ip=") {
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
        None
    }

    /// Parse `speck_gateway=IP` from the kernel command line.
    fn parse_cmdline_gateway(path: &str) -> Option<String> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(val) = word.strip_prefix("speck_gateway=") {
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
        None
    }

    /// Parse `docker_vsock_port=PORT` from the kernel command line.
    ///
    /// The default is 9003 when the key is absent.
    fn parse_cmdline_docker_vsock_port(path: &str) -> Option<u32> {
        let content = std::fs::read_to_string(path).ok()?;
        for word in content.split_whitespace() {
            if let Some(port_str) = word.strip_prefix("docker_vsock_port=") {
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

    /// Bind-mount /proc, /sys, /dev from the initrd namespace into /rootfs,
    /// and mount a fresh tmpfs at /rootfs/run.
    ///
    /// This is required before running dockerd (or any process) in a chroot
    /// rooted at /rootfs: the chroot'd process must be able to see proc, sys,
    /// and dev, which only exist in the outer namespace after
    /// `mount_early_filesystems()` runs.
    ///
    /// Call order: must run AFTER `mount_disks()` so that /rootfs is a
    /// valid ext4 mount point, and AFTER `mount_early_filesystems()` so
    /// that the bind sources (/proc, /sys, /dev) themselves exist.
    ///
    /// Mount failures are non-fatal — errors are logged and execution
    /// continues.  A chroot'd process that cannot see /proc will typically
    /// fail to start on its own; the error log is sufficient for diagnosis.
    fn mount_rootfs_runtime_filesystems() {
        // /rootfs/proc — bind from /proc
        let _ = std::fs::create_dir_all("/rootfs/proc");
        bind_mount("/proc", "/rootfs/proc");

        // /rootfs/sys — bind from /sys
        let _ = std::fs::create_dir_all("/rootfs/sys");
        bind_mount("/sys", "/rootfs/sys");

        // /rootfs/sys/fs/cgroup — mount cgroup2 hierarchy.
        // A plain MS_BIND of /sys does NOT carry cgroupv2 submounts;
        // crun sees sysfs type at /sys/fs/cgroup and rejects it with
        // "invalid file system type". Mount cgroup2 directly on top.
        let _ = std::fs::create_dir_all("/rootfs/sys/fs/cgroup");
        let ret = unsafe {
            libc::mount(
                b"cgroup2\0".as_ptr() as *const libc::c_char,
                b"/rootfs/sys/fs/cgroup\0".as_ptr() as *const libc::c_char,
                b"cgroup2\0".as_ptr() as *const libc::c_char,
                0,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: mount cgroup2 at /rootfs/sys/fs/cgroup failed: {:?}",
                io::Error::last_os_error()
            );
        } else {
            eprintln!("vminitd: mounted cgroup2 at /rootfs/sys/fs/cgroup");
        }

        // /rootfs/dev — bind from /dev
        let _ = std::fs::create_dir_all("/rootfs/dev");
        bind_mount("/dev", "/rootfs/dev");

        // /rootfs/run — fresh tmpfs (runtime sockets + pid files are ephemeral)
        let _ = std::fs::create_dir_all("/rootfs/run");
        let ret = unsafe {
            libc::mount(
                b"tmpfs\0".as_ptr() as *const libc::c_char,
                b"/rootfs/run\0".as_ptr() as *const libc::c_char,
                b"tmpfs\0".as_ptr() as *const libc::c_char,
                0,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: mount tmpfs at /rootfs/run failed: {:?}",
                io::Error::last_os_error()
            );
        }
    }

    /// Bind-mount `source` onto `target` using MS_BIND.
    ///
    /// Non-fatal: logs the error and returns.  The filesystem type is
    /// ignored by the kernel for bind mounts.
    fn bind_mount(source: &str, target: &str) {
        use std::ffi::CString;
        let src = CString::new(source).unwrap_or_default();
        let tgt = CString::new(target).unwrap_or_default();
        let ret = unsafe {
            libc::mount(
                src.as_ptr(),
                tgt.as_ptr(),
                std::ptr::null(),   // fstype ignored for MS_BIND
                libc::MS_BIND,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            eprintln!(
                "vminitd: bind mount {source} → {target} failed: {:?}",
                io::Error::last_os_error()
            );
        }
    }

    /// Mount the rootfs and data disks.
    ///
    /// 1. Mount /dev/vda (rootfs) at /rootfs as ext4.
    /// 2. Mount /dev/vdb (data disk) at /rootfs/var/lib/containerd as ext4.
    ///    If the data disk has no filesystem (EINVAL), format it with mke2fs
    ///    only after proving it has no filesystem signature, then retry.
    ///    Failure on the second attempt is fatal.
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
            if err.raw_os_error() == Some(libc::EINVAL) {
                if !data_disk_has_no_filesystem_signature("/dev/vdb") {
                    eprintln!(
                        "vminitd: /dev/vdb mount failed but disk is not proven blank; refusing to format: {:?}",
                        err
                    );
                    std::process::exit(1);
                }
                eprintln!("vminitd: /dev/vdb has no filesystem signature — formatting with mke2fs");
                let mke2fs_status = std::process::Command::new("/sbin/mke2fs")
                    .args(["-t", "ext4", "/dev/vdb"])
                    .status()
                    .expect("vminitd: failed to start /sbin/mke2fs");
                if !mke2fs_status.success() {
                    eprintln!("vminitd: mke2fs failed with {mke2fs_status}");
                    std::process::exit(1);
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

        grow_data_filesystem_if_needed("/dev/vdb", "/rootfs/var/lib/containerd");

        // Create runtime directories needed by containerd
        let _ = std::fs::create_dir_all("/rootfs/run");
        let _ = std::fs::create_dir_all("/rootfs/tmp");
    }

    /// Grow the mounted ext4 data filesystem to match the current block device size.
    ///
    /// This is fatal because continuing with the old filesystem size after the
    /// host grew `data.img` would silently violate the configured VM disk size.
    fn grow_data_filesystem_if_needed(device: &str, mountpoint: &str) {
        if !std::path::Path::new(mountpoint).exists() {
            eprintln!("vminitd: data filesystem mountpoint missing for {device}: {mountpoint}");
            std::process::exit(1);
        }

        let resize_tool = "/sbin/resize2fs";
        if !std::path::Path::new(resize_tool).exists() {
            eprintln!("vminitd: data filesystem resize tool missing or failed for {device}: {resize_tool} not found");
            std::process::exit(1);
        }

        eprintln!("vminitd: growing data filesystem on {device}");
        match run_resize_tool(resize_tool, &[device]) {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("vminitd: resize2fs failed with {status}");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("vminitd: data filesystem resize tool missing or failed for {device}: {e}");
                std::process::exit(1);
            }
        }
    }

    fn run_resize_tool(tool: &str, args: &[&str]) -> io::Result<std::process::ExitStatus> {
        std::process::Command::new(tool).args(args).status()
    }

    /// Return true only when a bounded signature probe positively reports that
    /// the data disk has no recognizable filesystem signature.
    ///
    /// Fail-safe semantics: recognized signatures, missing probe tools,
    /// execution failures, and ambiguous output all return false, which means
    /// the caller must not format the disk.
    fn data_disk_has_no_filesystem_signature(device: &str) -> bool {
        let output = match std::process::Command::new("/sbin/blkid")
            .arg(device)
            .output()
        {
            Ok(output) => output,
            Err(e) => {
                eprintln!("vminitd: failed to run /sbin/blkid for {device}: {e}");
                return false;
            }
        };

        if output.status.success() {
            eprintln!("vminitd: /sbin/blkid found a signature on {device}; refusing to format");
            return false;
        }

        let stdout_empty = output.stdout.iter().all(|b| b.is_ascii_whitespace());
        let stderr_empty = output.stderr.iter().all(|b| b.is_ascii_whitespace());

        match output.status.code() {
            Some(2) if stdout_empty && stderr_empty => true,
            code => {
                eprintln!(
                    "vminitd: /sbin/blkid did not prove {device} is blank (status: {:?}); refusing to format",
                    code
                );
                false
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Docker Engine socket readiness
    // ---------------------------------------------------------------------------

    /// Poll `/rootfs/run/speck/dockerd.sock` until dockerd accepts connections.
    ///
    /// Returns `true` if the socket became reachable within `max_attempts`
    /// (200 ms interval). Returns `false` if all attempts are exhausted.
    fn wait_for_dockerd_socket(max_attempts: u32) -> bool {
        for attempt in 0..max_attempts {
            if unix_connect_once("/rootfs/run/speck/dockerd.sock") {
                eprintln!("vminitd: dockerd socket ready after {attempt} attempts");
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

    /// Parse `speck_identity_tags=tag:path,tag:path` from the kernel cmdline.
    ///
    /// Returns a list of `(tag, guest_path)` pairs where `guest_path` is the
    /// absolute path the tag should be mounted at inside the guest rootfs.
    fn parse_cmdline_identity_tags(cmdline_path: &str) -> Vec<(String, String)> {
        let content = match std::fs::read_to_string(cmdline_path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        for word in content.split_whitespace() {
            if let Some(val) = word.strip_prefix("speck_identity_tags=") {
                return val
                    .split(',')
                    .filter_map(|pair| {
                        let mut parts = pair.splitn(2, ':');
                        let tag = parts.next()?.to_string();
                        let path = parts.next()?.to_string();
                        if tag.is_empty() || path.is_empty() {
                            return None;
                        }
                        Some((tag, path))
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    /// Mount identity root VirtioFS shares into `/rootfs` at their original paths.
    ///
    /// For each `(tag, path)` pair from `speck_identity_tags`, mounts the
    /// VirtioFS tag at `/rootfs<path>`.  For example, tag `speck-id-users`
    /// with path `/Users` is mounted at `/rootfs/Users`.
    ///
    /// Mount failures are non-fatal: logged and skipped.  The target directory
    /// is created if it does not exist.
    fn mount_identity_roots(cmdline_path: &str) {
        let tags = parse_cmdline_identity_tags(cmdline_path);
        if tags.is_empty() {
            return;
        }
        for (tag, guest_path) in &tags {
            // Strip a leading slash so we can join safely with /rootfs
            let relative = guest_path.trim_start_matches('/');
            let target = format!("/rootfs/{relative}");

            if let Err(e) = std::fs::create_dir_all(&target) {
                eprintln!("vminitd: create_dir_all {target} failed: {e}");
                continue;
            }

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
                    "vminitd: failed to mount identity root {tag} at {target}: {:?}",
                    io::Error::last_os_error()
                );
            } else {
                eprintln!("vminitd: mounted identity root {tag} at {target}");
            }
        }
        eprintln!("vminitd: mounted {} identity roots", tags.len());
    }

    // ---------------------------------------------------------------------------
    // Docker Engine backend
    // ---------------------------------------------------------------------------

    /// Return the dockerd binary path as it appears INSIDE the chroot (/rootfs).
    fn detect_dockerd_bin_in_chroot() -> &'static str {
        if std::path::Path::new("/rootfs/usr/bin/dockerd").exists() {
            "/usr/bin/dockerd"
        } else {
            "/usr/local/bin/dockerd"
        }
    }

    /// Spawn dockerd inside a chroot rooted at `/rootfs`, with automatic restart.
    ///
    /// Equivalent shell command:
    ///   chroot /rootfs <dockerd_bin> --host unix:///run/speck/dockerd.sock \
    ///       --data-root /var/lib/docker
    fn spawn_dockerd_with_restart(dockerd_bin_in_chroot: &'static str) {
        std::thread::spawn(move || {
            use std::os::unix::process::CommandExt as _;
            loop {
                let mut cmd = std::process::Command::new(dockerd_bin_in_chroot);
                cmd.args([
                    "--host", "unix:///run/speck/dockerd.sock",
                    "--data-root", "/var/lib/containerd",
                    "--iptables=false",
                    "--userland-proxy=false",
                ]);
                cmd.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");

                unsafe {
                    cmd.pre_exec(|| {
                        let ret = libc::chroot(b"/rootfs\0".as_ptr() as *const libc::c_char);
                        if ret < 0 { return Err(io::Error::last_os_error()); }
                        let ret = libc::chdir(b"/\0".as_ptr() as *const libc::c_char);
                        if ret < 0 { return Err(io::Error::last_os_error()); }
                        Ok(())
                    });
                }

                match cmd.spawn() {
                    Ok(mut child) => {
                        eprintln!("vminitd: started dockerd (pid {})", child.id());
                        match child.wait() {
                            Ok(status) => eprintln!("vminitd: dockerd exited {status} — restarting"),
                            Err(e) => eprintln!("vminitd: dockerd wait error: {e} — restarting"),
                        }
                    }
                    Err(e) => {
                        eprintln!("vminitd: failed to spawn dockerd: {e} — retrying");
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }

    // ---------------------------------------------------------------------------
    // Guest networking (static IP via ioctl)
    // ---------------------------------------------------------------------------

    /// Bring up the virtio-net interface with a static IP and default route.
    ///
    /// Uses `ioctl` syscalls exclusively — no external tools needed.  The guest
    /// IP and gateway are passed as strings parsed from the kernel cmdline
    /// (`speck_guest_ip=` and `speck_gateway=`).
    ///
    /// Steps:
    /// 1. Create a raw socket for ioctl.
    /// 2. Find the virtio-net interface (try eth0).
    /// 3. Set IFF_UP to bring the interface running.
    /// 4. Set IP address via SIOCSIFADDR.
    /// 5. Set netmask via SIOCSIFNETMASK (implicit /24).
    /// 6. Add default route via SIOCADDRT.
    ///
    /// Non-fatal: errors are logged and execution continues.  Without a live
    /// network, containers can only resolve DNS (via vsock proxy) but cannot
    /// pull images from external registries.
    /// Bring up the loopback interface so 127.0.0.1 is routed locally.
    ///
    /// Without this, packets destined for 127.0.0.1 fall through to the default
    /// route (eth0) because the kernel only adds 127.0.0.0/8 to the local routing
    /// table when `lo` is UP. DNS queries to 127.0.0.1:53 would otherwise exit
    /// the VM and time out.
    fn bring_up_loopback() {
        let sock_fd = match unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) } {
            fd if fd >= 0 => fd,
            _ => {
                eprintln!("vminitd: bring_up_loopback socket failed");
                return;
            }
        };

        let iface = b"lo\0";
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        for (i, &b) in iface.iter().enumerate() {
            if i < ifr.ifr_name.len() - 1 {
                ifr.ifr_name[i] = b;
            }
        }

        // Get current flags
        let ret = unsafe {
            libc::ioctl(sock_fd, libc::SIOCGIFFLAGS as libc::c_int, &ifr as *const libc::ifreq)
        };
        if ret < 0 {
            eprintln!("vminitd: lo SIOCGIFFLAGS failed: {:?}", std::io::Error::last_os_error());
            unsafe { libc::close(sock_fd) };
            return;
        }

        // Set IFF_UP | IFF_LOOPBACK
        unsafe {
            ifr.ifr_ifru.ifru_flags =
                (ifr.ifr_ifru.ifru_flags | (libc::IFF_UP as i16) | (libc::IFF_LOOPBACK as i16))
                    as i16;
        }
        let ret = unsafe {
            libc::ioctl(sock_fd, libc::SIOCSIFFLAGS as libc::c_int, &ifr as *const libc::ifreq)
        };
        if ret < 0 {
            eprintln!("vminitd: lo SIOCSIFFLAGS failed: {:?}", std::io::Error::last_os_error());
        } else {
            eprintln!("vminitd: loopback lo brought up");
        }

        unsafe { libc::close(sock_fd) };
    }

    fn bring_up_network(ip_str: &str, gw_str: &str) {
        let sock_fd = match unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) } {
            fd if fd >= 0 => fd,
            _ => {
                eprintln!("vminitd: bring_up_network socket failed");
                return;
            }
        };

        let iface = b"eth0\0";
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        for (i, &b) in iface.iter().enumerate() {
            if i < ifr.ifr_name.len() - 1 {
                ifr.ifr_name[i] = b;
            }
        }

        // --- Step 1: Bring the interface up via SIOCGIFFLAGS / SIOCSIFFLAGS ---
        // On aarch64-linux-musl the ioctl constants are u64 but the syscall takes
        // c_int; cast explicitly to suppress the type mismatch error.
        let flags_ret = unsafe {
            libc::ioctl(
                sock_fd,
                libc::SIOCGIFFLAGS as libc::c_int,
                &ifr as *const libc::ifreq,
            )
        };
        if flags_ret < 0 {
            eprintln!(
                "vminitd: SIOCGIFFLAGS failed: {:?}",
                io::Error::last_os_error()
            );
            unsafe { libc::close(sock_fd); }
            return;
        }
        unsafe {
            ifr.ifr_ifru.ifru_flags =
                (ifr.ifr_ifru.ifru_flags | (libc::IFF_UP as i16)) as i16;
        }
        let up_ret = unsafe {
            libc::ioctl(
                sock_fd,
                libc::SIOCSIFFLAGS as libc::c_int,
                &ifr as *const libc::ifreq,
            )
        };
        if up_ret < 0 {
            eprintln!(
                "vminitd: SIOCSIFFLAGS (IFF_UP) failed: {:?}",
                io::Error::last_os_error()
            );
            unsafe { libc::close(sock_fd); }
            return;
        }
        eprintln!("vminitd: brought up interface eth0");

        // --- Step 2: Parse IP to raw bytes ---
        let ip_bytes: [u8; 4] = match parse_ipv4(ip_str) {
            Some(b) => b,
            None => {
                eprintln!("vminitd: invalid guest IP: {ip_str}");
                unsafe { libc::close(sock_fd); }
                return;
            }
        };
        let gw_bytes: [u8; 4] = match parse_ipv4(gw_str) {
            Some(b) => b,
            None => {
                eprintln!("vminitd: invalid gateway: {gw_str}");
                unsafe { libc::close(sock_fd); }
                return;
            }
        };

        // --- Step 3: Set IP address via SIOCSIFADDR ---
        ifr.ifr_ifru.ifru_addr = libc::sockaddr {
            sa_family: libc::AF_INET as u16,
            // sa_data layout: [sin_port(2), sin_addr(4), sin_zero(8)]
            sa_data: [
                0, 0,
                ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3],
                0, 0, 0, 0, 0, 0, 0, 0,
            ],
        };
        let addr_ret = unsafe {
            libc::ioctl(
                sock_fd,
                libc::SIOCSIFADDR as libc::c_int,
                &ifr as *const libc::ifreq,
            )
        };
        if addr_ret < 0 {
            eprintln!(
                "vminitd: SIOCSIFADDR {ip_str} failed: {:?}",
                io::Error::last_os_error()
            );
            unsafe { libc::close(sock_fd); }
            return;
        }
        eprintln!("vminitd: set IP {ip_str} on eth0");

        // --- Step 4: Set netmask to 255.255.255.0 via SIOCSIFNETMASK ---
        ifr.ifr_ifru.ifru_addr = libc::sockaddr {
            sa_family: libc::AF_INET as u16,
            // sa_data layout: [sin_port(2), sin_addr(4), sin_zero(8)]
            sa_data: [
                0, 0,
                255u8, 255u8, 255u8, 0u8,
                0, 0, 0, 0, 0, 0, 0, 0,
            ],
        };
        let mask_ret = unsafe {
            libc::ioctl(
                sock_fd,
                libc::SIOCSIFNETMASK as libc::c_int,
                &ifr as *const libc::ifreq,
            )
        };
        if mask_ret < 0 {
            eprintln!(
                "vminitd: SIOCSIFNETMASK failed: {:?}",
                io::Error::last_os_error()
            );
            unsafe { libc::close(sock_fd); }
            return;
        }

        // --- Step 5: Add default route via SIOCADDRT ---
        let mut rt: libc::rtentry = unsafe { std::mem::zeroed() };
        let mut gw_sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        gw_sa.sin_family = libc::AF_INET as u16;
        gw_sa.sin_addr = libc::in_addr {
            // s_addr is network-byte-order bytes in memory; from_ne_bytes copies
            // the octets directly so [172,16,0,1] lands as-is on LE aarch64.
            s_addr: u32::from_ne_bytes(gw_bytes),
        };
        rt.rt_gateway = unsafe { std::mem::transmute(gw_sa) };
        rt.rt_dst = libc::sockaddr {
            sa_family: libc::AF_INET as u16,
            sa_data: [0; 14],
        };
        rt.rt_genmask = libc::sockaddr {
            sa_family: libc::AF_INET as u16,
            sa_data: [0; 14],
        };
        rt.rt_flags = (libc::RTF_UP | libc::RTF_GATEWAY) as u16;

        let route_ret = unsafe {
            libc::ioctl(sock_fd, libc::SIOCADDRT as libc::c_int, &rt as *const libc::rtentry)
        };
        if route_ret < 0 {
            let err = io::Error::last_os_error();
            // EEXIST means the route already exists (harmless)
            if err.raw_os_error() != Some(libc::EEXIST) {
                eprintln!("vminitd: SIOCADDRT default route via {gw_str} failed: {err:?}");
            }
        } else {
            eprintln!("vminitd: added default route via {gw_str}");
        }

        unsafe { libc::close(sock_fd); }
        eprintln!("vminitd: networking configured — eth0={ip_str}/24 gw={gw_str}");
    }

    /// Parse a dotted-quad IPv4 string into four bytes.
    fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 4 {
            return None;
        }
        let mut octets = [0u8; 4];
        for (i, p) in parts.iter().enumerate() {
            let val: u16 = p.parse().ok()?;
            if val > 255 {
                return None;
            }
            octets[i] = val as u8;
        }
        Some(octets)
    }
}

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("vminitd.rs");

    #[test]
    fn guest_resize_tooling_provisioning_path_exists() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("speck-guest lives under crates/speck-guest");
        let check_script = repo_root.join("scripts/check-guest-resize-tools.sh");
        let provision_script = repo_root.join("scripts/provision-guest-resize-tools.sh");

        let check_source = std::fs::read_to_string(&check_script)
            .expect("Phase 08 must include an initrd inspection gate script");
        let provision_source = std::fs::read_to_string(&provision_script)
            .expect("Phase 08 must include an initrd provisioning remediation script");

        assert!(
            SOURCE.contains("/sbin/resize2fs"),
            "Phase 08 must verify/provision /sbin/resize2fs before relying on it"
        );
        assert!(
            check_source.contains("/sbin/resize2fs")
                && check_source.contains("/sbin/mke2fs")
                && check_source.contains("/sbin/blkid"),
            "Inspection script must verify all exact required guest tool paths"
        );
        assert!(
            provision_source.contains("/sbin/resize2fs")
                && provision_source.contains("/sbin/mke2fs")
                && provision_source.contains("/sbin/blkid"),
            "Provisioning script must provision all exact required guest tool paths"
        );
        assert!(
            provision_source.contains("kata-alpine-3.22.initrd.pre-resize-tools.bak"),
            "Provisioning must preserve a pre-resize-tools initrd backup"
        );
    }

    #[test]
    fn mount_disks_grows_data_filesystem_before_runtime_dirs() {
        let data_mount = SOURCE
            .find("b\"/dev/vdb\\0\"")
            .expect("mount_disks should mount /dev/vdb");
        let grow_call = [
            "grow_data_filesystem_if_needed(\"/dev/vdb\"",
            ", \"/rootfs/var/lib/containerd\")",
        ]
        .concat();
        let grow = SOURCE
            .find(&grow_call)
            .expect("mount_disks should grow /dev/vdb after mounting it");
        let runtime_dir = SOURCE[grow..]
            .find("std::fs::create_dir_all(\"/rootfs/run\")")
            .map(|offset| grow + offset)
            .expect("mount_disks should create /rootfs/run");
        let boot_mount = SOURCE
            .find("mount_disks();")
            .expect("boot path should mount disks before services");
        let dockerd_spawn = SOURCE
            .find("spawn_dockerd_with_restart")
            .expect("boot path should support dockerd startup");

        assert!(data_mount < grow, "data filesystem must grow only after /dev/vdb mount succeeds");
        assert!(grow < runtime_dir, "data filesystem must grow before runtime directories are created");
        assert!(boot_mount < dockerd_spawn, "disk mounting/growth must happen before dockerd starts");
    }

    #[test]
    fn data_filesystem_growth_fails_closed_on_missing_resize_tool() {
        let helper = SOURCE
            .find("fn grow_data_filesystem_if_needed")
            .expect("guest data filesystem growth helper should exist");
        let resize_tool = SOURCE[helper..]
            .find("/sbin/resize2fs")
            .map(|offset| helper + offset)
            .expect("growth helper should invoke /sbin/resize2fs");
        let missing_or_failed = SOURCE[helper..]
            .find("vminitd: data filesystem resize tool missing or failed")
            .map(|offset| helper + offset)
            .expect("missing resize tool should emit an explicit vminitd diagnostic");
        let resize_failed = SOURCE[helper..]
            .find("vminitd: resize2fs failed")
            .map(|offset| helper + offset)
            .expect("unsuccessful resize2fs exit should emit an explicit diagnostic");
        let missing_fatal_exit = SOURCE[missing_or_failed..]
            .find("std::process::exit(1)")
            .map(|offset| missing_or_failed + offset)
            .expect("missing resize tool must fail closed by exiting PID 1");
        let failed_fatal_exit = SOURCE[resize_failed..]
            .find("std::process::exit(1)")
            .map(|offset| resize_failed + offset)
            .expect("failed resize2fs status must fail closed by exiting PID 1");

        assert!(helper < resize_tool, "helper should name the resize2fs tool it runs");
        assert!(resize_tool < missing_or_failed, "tool probe should happen before missing-tool diagnostic");
        assert!(resize_tool < resize_failed, "tool invocation should happen before failed-status diagnostic");
        assert!(missing_or_failed < missing_fatal_exit, "missing resize tool must exit non-zero after diagnostic");
        assert!(resize_failed < failed_fatal_exit, "failed resize2fs status must exit non-zero after diagnostic");
    }
}
