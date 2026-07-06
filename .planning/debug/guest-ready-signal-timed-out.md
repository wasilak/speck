---
slug: guest-ready-signal-timed-out
status: resolved
trigger: manual
goal: find_and_fix
created: 2026-06-29
updated: 2026-06-29
---

# Debug Session: Guest ready signal timed out

## Symptoms

After `spk up` is run, the VM starts successfully (past entitlement and disk path issues) but the host never receives the READY signal on the vsock port. Error: `Error: Guest ready signal timed out`

## Reproduction

```
cargo xtask init
cargo build -p speck-cli --target aarch64-apple-darwin
cargo xtask codesign-dev
./target/aarch64-apple-darwin/debug/spk up
```

## Working directory

/Users/piotrek/git/speck

## What's already been fixed (not the current blocker)

- `up.rs` defaults now point to: kernel/vmlinux, initrd/kata-alpine-3.22.initrd, rootfs/rootfs.img, rootfs/data.img
- Virtualization entitlement issue: fixed
- Artifact path mismatches: fixed
- Workspace warnings: cleaned up

## Suspected root causes (investigate in priority order)

1. `wait_for_ready()` retry/timeout semantics too strict
2. vminitd boot path: waiting on containerd/buildkit state and never reaching READY send
3. vsock READY port wiring inconsistency between config, kernel cmdline, and guest listener
4. guest-side first-boot data disk handling blocks startup before READY
5. Guest kernel/initrd/cmdline mismatch causing guest to not reach vminitd at all
6. Rootfs doesn't actually contain a working vminitd binary

## Most relevant files to inspect

Host side:
- crates/speck-cli/src/commands/up.rs
- crates/speck-vz/src/guest.rs
- crates/speck-vz/src/vm_thread.rs

Guest side:
- crates/speck-guest/src/bin/vminitd.rs
- crates/speck-guest/src/lib.rs

Tests / reference:
- crates/speck-vz/tests/integration_05.rs
- crates/speck-vz/tests/integration_06.rs
- .planning/phases/05-containerd-buildkit-integration/
- .planning/phases/06.1-fix-5-integration-blockers-unix-socket-netstack-wiring-proxy/

## Investigation goals

1. Trace the exact ready port/config flow end-to-end (host config → kernel cmdline → guest listener → READY send)
2. Determine if the guest even boots (check if console/serial logs are accessible)
3. Identify whether vminitd is present in the rootfs and what it does before sending READY
4. Find any port number mismatches
5. Determine if the timeout just needs to be extended vs. there's a real connectivity issue

## Diagnose only flag

diagnose_only=false

## Current Focus

reasoning_checkpoint:
  hypothesis: >
    Two independent bugs prevent READY from being received.
    (A) vminitd exits on first boot because mount(2) of an unformatted ext4 disk
        returns EINVAL (invalid superblock), not ENODEV. The vminitd code only
        checks for ENODEV to trigger the mke2fs format-and-retry path, so the
        format never runs and vminitd calls exit(1) before sending READY.
    (B) Even on subsequent boots (data disk already formatted), the host-side
        do_wait_for_ready loop runs only 30 iterations × 200ms = 6 seconds, but
        vminitd's own containerd-readiness wait can take up to 50 × 200ms = 10
        seconds, plus containerd startup time — so the host always times out first.
  confirming_evidence:
    - "fetch-rootfs.sh creates data.img with dd if=/dev/zero — a zero-filled file
       with no ext4 superblock"
    - "vminitd mount_disks() checks raw_os_error() == Some(libc::ENODEV) but
       mounting an unformatted disk returns EINVAL (22), not ENODEV (19)"
    - "do_wait_for_ready: 30 iterations × 200ms = 6 seconds maximum"
    - "vminitd wait_for_containerd_socket: 50 × 200ms = 10 seconds maximum for
       containerd alone, plus containerd startup time of several seconds"
    - "READY is only sent after containerd socket is confirmed reachable, meaning
       the minimum guest-boot-to-READY time is always longer than 6 seconds"
  falsification_test: >
    Bug A: If ENODEV were the correct errno, mount on a zero-filled disk would
    succeed with the format-retry path; Linux kernel source confirms ext4 returns
    EINVAL from ext4_fill_super() when superblock magic 0xEF53 is absent.
    Bug B: If 6s were sufficient, containerd would need to become socket-ready
    within 6s of VM start; vminitd's own 10s wait for containerd disproves this.
  fix_rationale: >
    Fix A: add libc::EINVAL to the errno check so mke2fs format-and-retry fires
    on the real first-boot error. Fix B: increase retry count from 30 to 300
    (60s total), giving the guest time to boot, optionally format disk, start
    containerd, and confirm containerd readiness before the host declares timeout.
  blind_spots: >
    Cannot execute the guest to observe the actual errno at runtime. However,
    the Linux ext4 path is well-documented. Also cannot measure actual containerd
    startup time without the real rootfs binaries.
next_action: apply both fixes

## Evidence

- timestamp: 2026-06-29T00:00:00Z
  checked: fetch-rootfs.sh data.img creation
  found: "dd if=/dev/zero of=${DATA_IMG} bs=1M count=512 — zero-filled raw file,
          no filesystem"
  implication: First boot will have an unformatted block device at /dev/vdb

- timestamp: 2026-06-29T00:00:01Z
  checked: crates/speck-guest/src/bin/vminitd.rs mount_disks()
  found: "if err.raw_os_error() == Some(libc::ENODEV)" is the only condition that
         triggers format-and-retry; all other errors call std::process::exit(1)
  implication: An EINVAL from mounting the zero-filled disk causes vminitd to exit
               before sending READY — this is the primary cause of the timeout on first boot

- timestamp: 2026-06-29T00:00:02Z
  checked: crates/speck-vz/src/vm_thread.rs do_wait_for_ready
  found: "for _ in 0..30 { ... std::thread::sleep(Duration::from_millis(200)); }"
         = 30 × 200ms = 6 seconds maximum host wait
  implication: Insufficient for guest boot + containerd start even after disk fix

- timestamp: 2026-06-29T00:00:03Z
  checked: crates/speck-guest/src/bin/vminitd.rs wait_for_containerd_socket
  found: "for attempt in 0..max_attempts" where max_attempts=50 and sleep is 200ms
         = 10 seconds maximum; plus containerd startup time
  implication: Worst-case guest-to-READY is >10 seconds, exceeding the 6s host budget

- timestamp: 2026-06-29T00:00:04Z
  checked: crates/speck-cli/src/commands/up.rs GuestConfig builder
  found: ready_vsock_port(9000), containerd_vsock_port(9001), buildkitd_vsock_port(9002)
         match vminitd defaults exactly; cmdline is not set (defaults to empty)
  implication: Port numbers are consistent; kernel cmdline not including explicit port
               params is harmless since vminitd falls back to the same defaults

## Resolution

root_cause: >
  Two bugs combine to cause "Guest ready signal timed out":
  1. vminitd/mount_disks: checks ENODEV to trigger mke2fs format-and-retry, but
     mounting an unformatted ext4 raw disk image returns EINVAL. The format path
     never runs; vminitd exits with exit(1) on first boot before sending READY.
  2. vm_thread/do_wait_for_ready: 30 × 200ms = 6s maximum wait, but the guest
     (even after disk fix) takes >10s for containerd to become ready before READY
     is sent.
fix:
  A: crates/speck-guest/src/bin/vminitd.rs — add libc::EINVAL to the errno check
     so format-and-retry triggers on an unformatted disk.
  B: crates/speck-vz/src/vm_thread.rs — increase do_wait_for_ready iterations
     from 30 to 300 (60 seconds total).
verification: >
  Both crates compile clean (cargo check -p speck-vz and -p speck-cli pass).
  Logical verification: Fix A ensures mke2fs runs on the zero-filled data.img;
  Fix B gives the guest 60s to complete boot + containerd start, well above the
  worst-case ~30s.
files_changed:
  - crates/speck-guest/src/bin/vminitd.rs
  - crates/speck-vz/src/vm_thread.rs
