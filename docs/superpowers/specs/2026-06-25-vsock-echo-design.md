# Vsock Echo: End-to-End Host↔Guest Communication

**Date:** 2026-06-25
**Status:** Approved
**Phase:** Phase 3 — vsock plumbing

## Architecture

```
Host (macOS)                          Guest (Linux micro-VM)
─────────────────                     ──────────────────────
Guest::start()
  └─ VZVirtioSocketDeviceConfig        vminitd (PID 1, static musl)
     └─ in VM config                     └─ AF_VSOCK socket
Guest::vsock_connect(1234)                 └─ bind(VMADDR_CID_ANY, 1234)
  └─ VZVirtioSocketDevice                  └─ listen() + accept()
     └─ connectToPort:completion:           └─ read/write echo loop
        └─ VZVirtioSocketConnection
           └─ raw fd (via fileDescriptor)
VzSocket
  └─ read/write/close
```

**Flow:** VM boots with custom initrd → vminitd starts, opens vsock listener → host detects `Running` → host connects via vsock → host sends bytes, guest echoes back → host verifies → clean shutdown.

## Host Side: `speck-vz`

### New file: `crates/speck-vz/src/vsock.rs`

- `VzSocket` — wraps the raw fd from `VZVirtioSocketConnection::fileDescriptor`
  - `read(buf)` → `Result<usize>` — delegates to `read(2)` on the fd
  - `write(buf)` → `Result<usize>` — delegates to `write(2)` on the fd
  - `Drop` — `close(2)` on the fd
- Constructor is internal (called from vm_thread); public API via `Guest::vsock_connect`

### Changes

**`Cargo.toml`** — enable features:
```
"VZVirtioSocketDeviceConfiguration",
"VZVirtioSocketDevice",
"VZVirtioSocketConnection",
```

**`vm_thread.rs`**:
- Add `VmControl.socket_device: Option<Retained<VZSocketDevice>>` — stored after creation in `do_start`
- In `do_start`: create `VZVirtioSocketDeviceConfiguration`, add to `vm_config.setSocketDevices(...)`
- Add `VmCommand::VsockConnect { port: u32, reply: oneshot::Sender<Result<i32, Error>> }` variant (returns raw fd)
- `do_vsock_connect(control, port)` → on dispatch queue: get socket device from control, call `connectToPort:completionHandler:`, extract fd, send back

**`config.rs`**:
- Add `vsock_port: u32` to `GuestConfig` (default `1234`)

**`guest.rs`**:
- Add `pub fn vsock_connect(&self) -> Result<VzSocket>`

**`lib.rs`**:
- Export `VzSocket`, make `vsock` module public

**`error.rs`**:
- Add `Error::VsockConnect(String)`, `Error::VsockTimeout`

## Guest Side: `speck-guest`

### Rewrite `crates/speck-guest/src/main.rs`

A single-threaded blocking vsock echo server:

```
main():
  - ignore SIGPIPE
  - svm = socket(AF_VSOCK, SOCK_STREAM, 0)
  - bind(svm, VMADDR_CID_ANY, 1234)
  - listen(svm, 1)
  - loop:
      conn = accept(svm)
      read/write echo loop until EOF
      close(conn)
```

**Dependencies:**
- `libc` crate for `AF_VSOCK`, `VMADDR_CID_ANY`, `sockaddr_vm`, socket/bind/listen/accept/read/write

### Cross-Compilation

Target: `aarch64-unknown-linux-musl`

```toml
# speck-guest/Cargo.toml
[dependencies]
libc = "0.2"
```

Build: `cargo build -p speck-guest --target aarch64-unknown-linux-musl --release`

### Initrd Construction

A simple script creates a minimal cpio archive containing the static vminitd binary:

```
#!/bin/bash
set -euo pipefail
SPECK_HOME="${SPECK_HOME:-$HOME/.local/share/speck}"
TARGET="aarch64-unknown-linux-musl/release/vminitd"
cargo build -p speck-guest --target aarch64-unknown-linux-musl --release
WORKDIR=$(mktemp -d)
cp "target/$TARGET" "$WORKDIR/vminitd"
chmod +x "$WORKDIR/vminitd"
cd "$WORKDIR"
find . | cpio -o -H newc | zstd -o "$SPECK_HOME/initrd/custom-vminitd.initrd"
rm -rf "$WORKDIR"
```

Kernel cmdline: `rdinit=/vminitd console=hvc0`

## Test: `test_vsock_echo`

In `crates/speck-vz/src/guest.rs` (or a new integration test):

1. Build config with custom initrd
2. Start VM, assert `Running`
3. Connect via `guest.vsock_connect(1234)`
4. Write `b"hello"`, read back, assert `b"hello"`
5. Close socket, stop VM

## Error Handling

- Vsock device creation failure → `Error::VmFramework` (validated at config time)
- `connectToPort` fails → `Error::VsockConnect(String)`
- connect times out (no guest listening) → `Error::VsockTimeout` (with a configurable timeout, default 5s)
- read/write errors → `Error::VsockIo(std::io::Error)`
- All vsock operations are done on the dispatch queue (same serial queue as VM commands)

## Future (not in this phase)

- gRPC/protobuf service definition and tonic server/client
- Multiple vsock ports for different services
- vminitd process supervision (containerd launch)
- Non-blocking I/O via tokio
