use speck_vz::{Error, Guest, GuestConfig};
use std::path::PathBuf;
use std::time::Duration;

fn kernel_path() -> PathBuf {
    std::env::var("SPEK_KERNEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            base.join("../../target/kernel/vmlinux")
        })
}

fn initrd_path() -> PathBuf {
    std::env::var("SPEK_INITRD")
        .map(PathBuf::from)
        .unwrap_or_else(|_| "/tmp/speck-initrd.cpio.gz".into())
}

#[test]
#[ignore = "requires VM entitlement + kernel + initrd artifacts"]
fn test_vsock_echo() {
    let config = GuestConfig::builder()
        .kernel_path(kernel_path())
        .initrd_path(initrd_path())
        .cmdline("console=hvc0 vsock_port=1234")
        .cpu_count(1)
        .memory_size_bytes(512 * 1024 * 1024)
        .stop_timeout(Duration::from_secs(10))
        .build();
    let guest = Guest::new(config);
    guest.start().expect("guest start");

    std::thread::sleep(Duration::from_secs(3));

    let sock = guest.vsock_connect(1234).expect("vsock_connect");
    let payload = b"hello speck";
    let n_written = sock.write(payload).expect("write");
    assert_eq!(n_written, payload.len());

    let mut buf = [0u8; 4096];
    let n_read = sock.read(&mut buf).expect("read");
    assert_eq!(&buf[..n_read], payload);

    drop(sock);
    guest.stop().expect("guest stop");
}

#[test]
#[ignore = "requires VM entitlement"]
fn test_vsock_connect_refused() {
    let config = GuestConfig::builder()
        .kernel_path(kernel_path())
        .initrd_path(initrd_path())
        .cmdline("console=hvc0 vsock_port=1234")
        .cpu_count(1)
        .memory_size_bytes(512 * 1024 * 1024)
        .stop_timeout(Duration::from_secs(10))
        .build();
    let guest = Guest::new(config);
    guest.start().expect("guest start");

    std::thread::sleep(Duration::from_secs(3));

    let result = guest.vsock_connect(9999);
    assert!(
        matches!(result, Err(Error::VsockTimeout)),
        "expected VsockTimeout, got {:?}",
        result
    );

    guest.stop().expect("guest stop");
}
