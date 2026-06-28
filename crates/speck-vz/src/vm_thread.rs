use std::ops::{Deref, DerefMut};
use std::os::fd::AsRawFd;
use std::os::unix::io::RawFd;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

use block2::{RcBlock, StackBlock};
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::AnyThread;
use objc2::ffi::NSInteger;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSArray, NSError, NSFileHandle, NSString, NSURL};
use objc2_virtualization::{
    VZDiskImageStorageDeviceAttachment, VZEntropyDeviceConfiguration,
    VZFileHandleNetworkDeviceAttachment, VZGenericPlatformConfiguration, VZLinuxBootLoader,
    VZMACAddress, VZNetworkDeviceAttachment, VZNetworkDeviceConfiguration, VZSocketDevice,
    VZSocketDeviceConfiguration, VZStorageDeviceAttachment, VZStorageDeviceConfiguration,
    VZVirtioBlockDeviceConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioNetworkDeviceConfiguration, VZVirtioSocketConnection, VZVirtioSocketDevice,
    VZVirtioSocketDeviceConfiguration, VZVirtualMachine, VZVirtualMachineConfiguration,
};
use socket2::{Domain, Socket, Type};

use crate::config::GuestConfig;
use crate::config::PortMapConfig;
use crate::delegate::{VmDelegate, VmStateEvent};
use crate::error::Error;
use crate::vsock::VzSocket;

type ReplySender = Option<oneshot::Sender<std::result::Result<InternalState, Error>>>;

/// Wrapper around `Option<Retained<VZVirtualMachine>>` that is explicitly `Send`.
///
/// SAFETY: `VZVirtualMachine` is only ever accessed from the serial dispatch queue,
/// so holding the retained reference in a `Mutex`-guarded field is safe as long as
/// all access happens on that queue.
struct VmMachine {
    inner: Option<Retained<VZVirtualMachine>>,
}

unsafe impl Send for VmMachine {}

impl Deref for VmMachine {
    type Target = Option<Retained<VZVirtualMachine>>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for VmMachine {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Wrapper around `Option<Retained<VZSocketDevice>>` that is explicitly `Send`.
///
/// SAFETY: `VZSocketDevice` is only ever accessed from the serial dispatch queue,
/// so holding the retained reference in a `Mutex`-guarded field is safe as long as
/// all access happens on that queue.
struct VmSocketDevice {
    inner: Option<Retained<VZSocketDevice>>,
}

unsafe impl Send for VmSocketDevice {}

impl Deref for VmSocketDevice {
    type Target = Option<Retained<VZSocketDevice>>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for VmSocketDevice {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub(crate) enum VmCommand {
    Start {
        config: GuestConfig,
        reply: oneshot::Sender<std::result::Result<InternalState, Error>>,
    },
    Stop {
        stop_timeout: Duration,
        reply: oneshot::Sender<std::result::Result<InternalState, Error>>,
    },
    State {
        reply: oneshot::Sender<InternalState>,
    },
    VsockConnect {
        port: u32,
        reply: oneshot::Sender<std::result::Result<VzSocket, Error>>,
    },
    NetstackFd {
        reply: oneshot::Sender<std::result::Result<RawFd, Error>>,
    },
    DnsVsockFd {
        reply: oneshot::Sender<std::result::Result<RawFd, Error>>,
    },
    WaitForGuestReady {
        ready_vsock_port: u32,
        reply: oneshot::Sender<std::result::Result<(), Error>>,
    },
    AddPortMap {
        host_port: u16,
        container_port: u16,
        reply: oneshot::Sender<std::result::Result<(), Error>>,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalState {
    Stopped,
    Starting,
    Running,
    Stopping,
}

impl From<InternalState> for speck_core::VmState {
    fn from(s: InternalState) -> Self {
        match s {
            InternalState::Stopped => speck_core::VmState::Stopped,
            InternalState::Starting => speck_core::VmState::Starting,
            InternalState::Running => speck_core::VmState::Running,
            InternalState::Stopping => speck_core::VmState::Stopping,
        }
    }
}

struct VmControl {
    state: InternalState,
    machine: VmMachine,
    socket_device: VmSocketDevice,
    netstack_fd: Option<RawFd>,
    dns_vsock_fd: Option<RawFd>,
    port_maps: Vec<PortMapConfig>,
    port_map_tx: Option<mpsc::Sender<PortMapConfig>>,
}

pub struct VmThread {
    sender: mpsc::Sender<VmCommand>,
    thread: Option<JoinHandle<()>>,
}

unsafe impl Send for VmThread {}
unsafe impl Sync for VmThread {}

impl VmThread {
    pub fn spawn() -> Self {
        let (tx, mut rx) = mpsc::channel::<VmCommand>(16);
        let queue = DispatchQueue::new("com.speck.vm", DispatchQueueAttr::SERIAL);
        let control = Arc::new(Mutex::new(VmControl {
            state: InternalState::Stopped,
            machine: VmMachine { inner: None },
            socket_device: VmSocketDevice { inner: None },
            netstack_fd: None,
            dns_vsock_fd: None,
            port_maps: Vec::new(),
            port_map_tx: None,
        }));

        let thread = thread::Builder::new()
            .name("speck-vm".into())
            .spawn(move || {
                while let Some(cmd) = rx.blocking_recv() {
                    match cmd {
                        VmCommand::Start { config, reply } => {
                            let control = Arc::clone(&control);
                            let reply = Arc::new(Mutex::new(Some(reply)));
                            let q = queue.clone();
                            queue.exec_sync(move || {
                                if let Err(e) = Self::do_start(&control, &reply, &config, &q) {
                                    let mut guard = reply.lock().unwrap_or_else(|e| e.into_inner());
                                    if let Some(sender) = guard.take() {
                                        let _ = sender.send(Err(e));
                                    }
                                }
                            });
                        }
                        VmCommand::Stop {
                            stop_timeout,
                            reply,
                        } => {
                            let control = Arc::clone(&control);
                            let q = queue.clone();
                            let result = Self::do_stop(&control, &q, stop_timeout);
                            let _ = reply.send(result);
                        }
                        VmCommand::State { reply } => {
                            let control = Arc::clone(&control);
                            queue.exec_sync(move || {
                                let ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
                                let _ = reply.send(ctrl.state);
                            });
                        }
                        VmCommand::VsockConnect { port, reply } => {
                            let control = Arc::clone(&control);
                            let q = queue.clone();
                            let result = Self::do_vsock_connect(&control, &q, port);
                            let _ = reply.send(result);
                        }
                        VmCommand::NetstackFd { reply } => {
                            let ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(fd) = ctrl.netstack_fd {
                                let dup_fd = unsafe { libc::dup(fd) };
                                if dup_fd < 0 {
                                    let _ = reply.send(Err(Error::NetworkIo(
                                        std::io::Error::last_os_error(),
                                    )));
                                } else {
                                    let _ = reply.send(Ok(dup_fd));
                                }
                            } else {
                                let _ = reply
                                    .send(Err(Error::Network("no netstack fd available".into())));
                            }
                        }
                        VmCommand::DnsVsockFd { reply } => {
                            let ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(fd) = ctrl.dns_vsock_fd {
                                let dup_fd = unsafe { libc::dup(fd) };
                                if dup_fd < 0 {
                                    let _ = reply.send(Err(Error::NetworkIo(
                                        std::io::Error::last_os_error(),
                                    )));
                                } else {
                                    let _ = reply.send(Ok(dup_fd));
                                }
                            } else {
                                let _ = reply
                                    .send(Err(Error::Network("no dns vsock fd available".into())));
                            }
                        }
                        VmCommand::WaitForGuestReady {
                            ready_vsock_port,
                            reply,
                        } => {
                            let control = Arc::clone(&control);
                            let q = queue.clone();
                            let result = Self::do_wait_for_ready(&control, &q, ready_vsock_port);
                            let _ = reply.send(result);
                        }
                        VmCommand::AddPortMap {
                            host_port,
                            container_port,
                            reply,
                        } => {
                            let port_map = PortMapConfig {
                                host_port,
                                container_port,
                            };
                            let sender = {
                                let mut ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
                                if ctrl.state != InternalState::Running {
                                    let _ = reply.send(Err(Error::NotRunning));
                                    continue;
                                }
                                ctrl.port_maps.push(port_map);
                                ctrl.port_map_tx.clone()
                            };

                            if let Some(tx) = sender {
                                let result = tx.blocking_send(port_map).map_err(|_| {
                                    Error::Network("netstack port-map channel closed".into())
                                });
                                let _ = reply.send(result);
                            } else {
                                let _ = reply.send(Ok(()));
                            }
                        }
                        VmCommand::Shutdown => break,
                    }
                }
            })
            .expect("spawning speck-vm worker thread");

        Self {
            sender: tx,
            thread: Some(thread),
        }
    }

    fn do_start(
        control: &Arc<Mutex<VmControl>>,
        reply: &Arc<Mutex<ReplySender>>,
        config: &GuestConfig,
        queue: &DispatchQueue,
    ) -> Result<(), Error> {
        let mut ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
        if ctrl.state != InternalState::Stopped {
            return Err(Error::AlreadyRunning);
        }
        ctrl.state = InternalState::Starting;
        ctrl.machine.inner = None;
        drop(ctrl);

        let kernel_str = NSString::from_str(
            config
                .kernel_path
                .to_str()
                .ok_or_else(|| Error::VmFramework("non-UTF-8 kernel path".into()))?,
        );
        let virtiofs_cmdline = crate::virtiofs::cmdline_virtiofs_arg(
            &config.volume_mounts,
            &config.speck_home,
        );

        let bootloader = unsafe {
            let kernel_url = NSURL::fileURLWithPath(&kernel_str);
            let bl = VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel_url);

            let full_cmdline = if config.cmdline.is_empty() {
                virtiofs_cmdline.clone()
            } else if virtiofs_cmdline.is_empty() {
                config.cmdline.clone()
            } else {
                format!("{} {}", config.cmdline, virtiofs_cmdline)
            };
            if !full_cmdline.is_empty() {
                let cmdline = NSString::from_str(&full_cmdline);
                bl.setCommandLine(&cmdline);
            }
            if let Some(ref initrd) = config.initrd_path {
                let initrd_str = NSString::from_str(
                    initrd
                        .to_str()
                        .ok_or_else(|| Error::VmFramework("non-UTF-8 initrd path".into()))?,
                );
                let initrd_url = NSURL::fileURLWithPath(&initrd_str);
                bl.setInitialRamdiskURL(Some(&initrd_url));
            }

            bl
        };

        let network = config.network.clone();
        let mut dup_host_fd: Option<RawFd> = None;

        let (vm_config, _platform, _entropy, _vsock_container) = unsafe {
            let vm_config =
                VZVirtualMachineConfiguration::init(VZVirtualMachineConfiguration::alloc());
            vm_config.setBootLoader(Some(&bootloader));
            vm_config.setCPUCount(
                config
                    .cpu_count
                    .try_into()
                    .map_err(|_| Error::VmFramework("CPU count overflow".into()))?,
            );
            vm_config.setMemorySize(config.memory_size_bytes);

            let platform =
                VZGenericPlatformConfiguration::init(VZGenericPlatformConfiguration::alloc());
            vm_config.setPlatform(&platform);

            let entropy = VZVirtioEntropyDeviceConfiguration::init(
                VZVirtioEntropyDeviceConfiguration::alloc(),
            );
            let entropy_ref: &VZEntropyDeviceConfiguration = &entropy;
            let entropy_array = NSArray::from_slice(&[entropy_ref]);
            vm_config.setEntropyDevices(&entropy_array);

            let vsock =
                VZVirtioSocketDeviceConfiguration::init(VZVirtioSocketDeviceConfiguration::alloc());
            let vsock_ref: &VZSocketDeviceConfiguration = &vsock;
            let socket_array = NSArray::from_slice(&[vsock_ref]);
            vm_config.setSocketDevices(&socket_array);

            // ── Network device (Virtio + file handle attachment) ──────────
            if let Some(ref net) = network {
                let sockets =
                    Socket::pair(Domain::UNIX, Type::DGRAM, None).map_err(Error::NetworkIo)?;
                let (host_socket, vm_socket) = (sockets.0, sockets.1);

                host_socket
                    .set_nonblocking(true)
                    .map_err(Error::NetworkIo)?;

                // dup the vm-facing fd for the NSFileHandle
                let vm_fd = vm_socket.as_raw_fd();
                let dup_vm_fd = libc::dup(vm_fd);
                if dup_vm_fd < 0 {
                    return Err(Error::NetworkIo(std::io::Error::last_os_error()));
                }

                let file_handle =
                    NSFileHandle::initWithFileDescriptor(NSFileHandle::alloc(), dup_vm_fd);

                let attachment = VZFileHandleNetworkDeviceAttachment::initWithFileHandle(
                    VZFileHandleNetworkDeviceAttachment::alloc(),
                    &file_handle,
                );

                // Only set the MTU when it differs from the default (1500)
                if net.mtu > 1500 {
                    attachment.setMaximumTransmissionUnit(net.mtu as NSInteger);
                }

                let virtio_net = VZVirtioNetworkDeviceConfiguration::init(
                    VZVirtioNetworkDeviceConfiguration::alloc(),
                );
                let attachment_ref: &VZNetworkDeviceAttachment = &attachment;
                virtio_net.setAttachment(Some(attachment_ref));

                // Convert the raw MAC bytes to a VZMACAddress via string.
                let mac_str = format!(
                    "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    net.mac[0], net.mac[1], net.mac[2], net.mac[3], net.mac[4], net.mac[5],
                );
                let mac_ns = NSString::from_str(&mac_str);
                let mac_addr = VZMACAddress::initWithString(VZMACAddress::alloc(), &mac_ns)
                    .ok_or_else(|| Error::VmFramework("invalid MAC address".into()))?;
                virtio_net.setMACAddress(&mac_addr);

                let net_ref: &VZNetworkDeviceConfiguration = &virtio_net;
                let net_array = NSArray::from_slice(&[net_ref]);
                vm_config.setNetworkDevices(&net_array);

                // Dup the host-facing fd for storage (original closes on drop).
                let host_fd = host_socket.as_raw_fd();
                let d = libc::dup(host_fd);
                if d < 0 {
                    return Err(Error::NetworkIo(std::io::Error::last_os_error()));
                }
                dup_host_fd = Some(d);
            }

            // ── Storage devices: rootfs (/dev/vda) first, data disk (/dev/vdb) second
            //    — ordering determines guest device names (Pitfall 4) ─────────────
            let (_rootfs_block, _data_block) = if let (Some(rootfs_path), Some(data_path)) =
                (&config.rootfs_disk_path, &config.data_disk_path)
            {
                let rootfs_block = Self::make_block_device(rootfs_path, false)?;
                let data_block = Self::make_block_device(data_path, false)?;
                let disks: &[&VZStorageDeviceConfiguration] = &[&rootfs_block, &data_block];
                let storage_array = NSArray::from_slice(disks);
                vm_config.setStorageDevices(&storage_array);
                (Some(rootfs_block), Some(data_block))
            } else {
                (None, None)
            };

            // ── VirtioFS directory sharing devices ──────────────────────
            if !config.volume_mounts.is_empty() || config.network.is_some() {
                crate::virtiofs::configure_virtiofs_devices(
                    &vm_config,
                    &config.volume_mounts,
                    &config.speck_home,
                )?;
            }

            Result::<_, Error>::Ok((vm_config, platform, entropy, vsock))
        }?;

        if let Err(error) = unsafe { vm_config.validateWithError() } {
            let (mut ctrl, desc) = autoreleasepool(|pool| {
                let ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
                let desc = error.localizedDescription();
                let desc = unsafe { desc.to_str(pool).to_string() };
                (ctrl, desc)
            });
            ctrl.state = InternalState::Stopped;
            return Err(Error::VmFramework(format!("config validation: {desc}")));
        }

        let (delegate_tx, _delegate_rx) = std::sync::mpsc::channel::<VmStateEvent>();
        let delegate = VmDelegate::create(delegate_tx);

        let vm = unsafe {
            VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &vm_config,
                queue,
            )
        };
        unsafe {
            vm.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }

        {
            let mut ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
            ctrl.machine.inner = Some(vm);

            // Extract the vsock socket device so we can connect to ports later
            if let Some(ref vm) = *ctrl.machine {
                let devices = unsafe { vm.socketDevices() };
                if let Some(device) = devices.firstObject() {
                    ctrl.socket_device.inner = Some(device);
                }
            }

            // Store the netstack fd so consumers can retrieve it
            if network.is_some() {
                ctrl.netstack_fd = dup_host_fd;
            }
        }

        let dns_vsock_port = config.dns_vsock_port;
        let initial_port_maps = config.port_maps.clone();
        let control_for_block = Arc::clone(control);
        let reply_for_block = Arc::clone(reply);
        let block = RcBlock::new(move |error: *mut NSError| {
            autoreleasepool(|pool| {
                if error.is_null() {
                    let mut ctrl = control_for_block.lock().unwrap_or_else(|e| e.into_inner());
                    ctrl.state = InternalState::Running;
                    ctrl.port_maps = initial_port_maps.clone();
                    let mut guard = reply_for_block.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(sender) = guard.take() {
                        let _ = sender.send(Ok(InternalState::Running));
                    }

                    // ── vsock DNS connection (D-07) ────────────────────────
                    let dns_device = ctrl.socket_device.inner.clone();
                    drop(ctrl);
                    if let Some(dns_port) = dns_vsock_port {
                        let control_for_dns = Arc::clone(&control_for_block);
                        if let Some(device) = dns_device {
                            match device.downcast::<VZVirtioSocketDevice>() {
                                Ok(vsock) => {
                                    let dns_block = StackBlock::new(
                                        move |conn: *mut VZVirtioSocketConnection,
                                              _err: *mut NSError| {
                                        if !conn.is_null() {
                                            let raw_fd = unsafe { (*conn).fileDescriptor() };
                                            let dup_fd = unsafe { libc::dup(raw_fd) };
                                            if dup_fd < 0 {
                                                // Best-effort; log skipped.
                                            } else {
                                                let mut c = control_for_dns
                                                    .lock()
                                                    .unwrap_or_else(|e| e.into_inner());
                                                c.dns_vsock_fd = Some(dup_fd);
                                            }
                                        }
                                    },
                                    );
                                    unsafe {
                                        vsock.connectToPort_completionHandler(dns_port, &dns_block);
                                    }
                                }
                                Err(_) => {
                                    // Not a VZVirtioSocketDevice — best-effort skip.
                                }
                            }
                        }
                    }
                } else {
                    let ns_error = unsafe { &*error };
                    let desc = ns_error.localizedDescription();
                    let err_str = unsafe { desc.to_str(pool).to_string() };
                    let mut ctrl = control_for_block.lock().unwrap_or_else(|e| e.into_inner());
                    ctrl.state = InternalState::Stopped;
                    let mut guard = reply_for_block.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(sender) = guard.take() {
                        let _ = sender.send(Err(Error::VmFramework(err_str)));
                    }
                }
            });
        });

        if let Some(vm) = &*control.lock().unwrap_or_else(|e| e.into_inner()).machine {
            unsafe { vm.startWithCompletionHandler(&block) };
        }

        Ok(())
    }

    fn do_stop(
        control: &Arc<Mutex<VmControl>>,
        queue: &DispatchQueue,
        stop_timeout: Duration,
    ) -> std::result::Result<InternalState, Error> {
        {
            let ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
            if ctrl.state != InternalState::Running {
                return Err(Error::NotRunning);
            }
        }

        let (done_tx, done_rx) = std::sync::mpsc::channel::<std::result::Result<(), Error>>();
        let control_for_block = Arc::clone(control);
        let done_tx_for_block = done_tx.clone();

        queue.exec_sync(move || {
            let machine = {
                let mut ctrl = control_for_block.lock().unwrap_or_else(|e| e.into_inner());
                ctrl.state = InternalState::Stopping;
                ctrl.machine.inner.take()
            };

            let control_for_block2 = Arc::clone(&control_for_block);
            let done_tx_block = done_tx_for_block.clone();
            let block = RcBlock::new(move |error: *mut NSError| {
                autoreleasepool(|pool| {
                    let mut ctrl = control_for_block2.lock().unwrap_or_else(|e| e.into_inner());
                    if error.is_null() {
                        ctrl.state = InternalState::Stopped;
                        let _ = done_tx_block.send(Ok(()));
                    } else {
                        let ns_error = unsafe { &*error };
                        let desc = ns_error.localizedDescription();
                        let err_str = unsafe { desc.to_str(pool).to_string() };
                        ctrl.state = InternalState::Running;
                        let _ = done_tx_block.send(Err(Error::VmFramework(err_str)));
                    }
                });
            });

            if let Some(vm) = &machine {
                unsafe { vm.stopWithCompletionHandler(&block) };
            } else {
                let _ = done_tx_for_block.send(Err(Error::NotRunning));
            }
        });

        match done_rx.recv_timeout(stop_timeout) {
            Ok(Ok(())) => Ok(InternalState::Stopped),
            Ok(Err(e)) => {
                let mut ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
                ctrl.state = InternalState::Stopped;
                Err(e)
            }
            Err(_) => {
                let mut ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
                ctrl.state = InternalState::Stopped;
                Err(Error::StopTimeout)
            }
        }
    }

    /// Create a `VZVirtioBlockDeviceConfiguration` from a disk image path.
    ///
    /// The returned block device is backed by a `VZDiskImageStorageDeviceAttachment`
    /// and can be added to `VZVirtualMachineConfiguration` via `setStorageDevices`.
    /// Disk ordering determines guest device names: first attached → `/dev/vda`,
    /// second → `/dev/vdb`, etc.
    fn make_block_device(
        path: &std::path::Path,
        read_only: bool,
    ) -> std::result::Result<Retained<VZVirtioBlockDeviceConfiguration>, Error> {
        let path_str = NSString::from_str(
            path.to_str()
                .ok_or_else(|| Error::DiskAttachment("non-UTF-8 disk path".into()))?,
        );
        let url = NSURL::fileURLWithPath(&path_str);
        let attachment = unsafe {
            VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
                VZDiskImageStorageDeviceAttachment::alloc(),
                &url,
                read_only,
            )
            .map_err(|_| Error::DiskAttachment("failed to create disk attachment".into()))?
        };
        let block_dev = unsafe {
            VZVirtioBlockDeviceConfiguration::initWithAttachment(
                VZVirtioBlockDeviceConfiguration::alloc(),
                &attachment as &VZStorageDeviceAttachment,
            )
        };
        Ok(block_dev)
    }

    fn do_vsock_connect(
        control: &Arc<Mutex<VmControl>>,
        queue: &DispatchQueue,
        port: u32,
    ) -> Result<VzSocket, Error> {
        // Quick check that a socket device exists before dispatching to the queue.
        {
            let ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
            if ctrl.socket_device.inner.is_none() {
                return Err(Error::VsockConnect("VM has no vsock socket device".into()));
            }
        }

        let (done_tx, done_rx) = std::sync::mpsc::channel::<Result<VzSocket, Error>>();

        queue.exec_sync(move || {
            let ctrl = control.lock().unwrap_or_else(|e| e.into_inner());
            let device = ctrl.socket_device.inner.as_ref().expect("checked above");

            // Clone so we can downcast without taking from ctrl.
            let device_clone = device.clone();
            match device_clone.downcast::<VZVirtioSocketDevice>() {
                Ok(vsock) => {
                    // Clone the sender for the block; the original stays for error path.
                    let done_tx_for_block = done_tx.clone();
                    let block = StackBlock::new(
                        move |connection: *mut VZVirtioSocketConnection, error: *mut NSError| {
                            if !connection.is_null() {
                                let conn = unsafe { &*connection };
                                let raw_fd = unsafe { conn.fileDescriptor() };
                                // dup the fd — the connection owns the original and
                                // will close it on dealloc.
                                let dup_fd = unsafe { libc::dup(raw_fd) };
                                if dup_fd < 0 {
                                    let io_err = std::io::Error::last_os_error();
                                    let _ = done_tx_for_block.send(Err(Error::VsockIo(io_err)));
                                } else {
                                    let sock = unsafe { VzSocket::from_raw_fd(dup_fd) };
                                    let _ = done_tx_for_block.send(Ok(sock));
                                }
                            } else if !error.is_null() {
                                autoreleasepool(|pool| {
                                    let ns_error = unsafe { &*error };
                                    let desc = ns_error.localizedDescription();
                                    let err_str = unsafe { desc.to_str(pool).to_string() };
                                    let _ =
                                        done_tx_for_block.send(Err(Error::VsockConnect(err_str)));
                                });
                            } else {
                                let _ = done_tx_for_block.send(Err(Error::VsockConnect(
                                    "no connection and no error".into(),
                                )));
                            }
                        },
                    );

                    unsafe { vsock.connectToPort_completionHandler(port, &block) };
                }
                Err(_) => {
                    let _ = done_tx.send(Err(Error::VsockConnect(
                        "socket device is not a VZVirtioSocketDevice".into(),
                    )));
                }
            }
        });

        match done_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(result) => result,
            Err(_) => Err(Error::VsockTimeout),
        }
    }

    /// Poll the guest's ready vsock port until the b"READY\n" signal arrives.
    ///
    /// Retries up to 30 times with 200 ms sleep between attempts (~6 s max).
    /// ECONNREFUSED / VsockTimeout are expected until vminitd binds the port.
    fn do_wait_for_ready(
        control: &Arc<Mutex<VmControl>>,
        queue: &DispatchQueue,
        ready_vsock_port: u32,
    ) -> Result<(), Error> {
        for _ in 0..30 {
            match Self::do_vsock_connect(control, queue, ready_vsock_port) {
                Ok(sock) => {
                    let mut buf = [0u8; 6];
                    let mut n = 0;
                    while n < 6 {
                        match sock.read(&mut buf[n..]) {
                            Ok(0) => break, // EOF before full signal
                            Ok(read) => n += read,
                            Err(e) => return Err(Error::VsockIo(e)),
                        }
                    }
                    if &buf == b"READY\n" {
                        return Ok(());
                    } else {
                        return Err(Error::VsockConnect("unexpected READY signal".into()));
                    }
                }
                Err(Error::VsockConnect(_)) | Err(Error::VsockTimeout) => {
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => return Err(e),
            }
        }
        Err(Error::GuestReadyTimeout)
    }

    fn send_blocking<T>(
        &self,
        cmd: VmCommand,
        rx: oneshot::Receiver<T>,
    ) -> std::result::Result<T, Error> {
        self.sender
            .blocking_send(cmd)
            .map_err(|_| Error::ChannelError("vm thread channel closed".into()))?;
        rx.blocking_recv()
            .map_err(|_| Error::ChannelError("vm thread reply channel closed".into()))
    }

    pub fn start(&self, config: GuestConfig) -> std::result::Result<InternalState, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(VmCommand::Start { config, reply: tx }, rx)?
    }

    pub fn stop(&self, stop_timeout: Duration) -> std::result::Result<InternalState, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(
            VmCommand::Stop {
                stop_timeout,
                reply: tx,
            },
            rx,
        )?
    }

    pub(crate) fn send_shutdown(&self) -> std::result::Result<(), Error> {
        self.sender
            .blocking_send(VmCommand::Shutdown)
            .map_err(|_| Error::ChannelError("vm thread channel closed".into()))
    }

    pub fn state(&self) -> std::result::Result<InternalState, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(VmCommand::State { reply: tx }, rx)
    }

    #[allow(dead_code)]
    pub fn state_as_core(&self) -> std::result::Result<speck_core::VmState, Error> {
        self.state().map(InternalState::into)
    }

    pub fn vsock_connect(&self, port: u32) -> std::result::Result<VzSocket, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(VmCommand::VsockConnect { port, reply: tx }, rx)?
    }

    /// Retrieve the raw file descriptor for the host side of the netstack socketpair.
    ///
    /// The returned fd is a **dup** of the internal one; the caller owns it and
    /// must close it when done. Returns an error if networking was not configured
    /// or the VM is not running.
    pub fn netstack_fd(&self) -> std::result::Result<RawFd, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(VmCommand::NetstackFd { reply: tx }, rx)?
    }

    /// Retrieve the raw file descriptor for the vsock DNS connection.
    ///
    /// The returned fd is a **dup** of the internal one; the caller owns it and
    /// must close it when done. Returns an error if no DNS vsock connection was
    /// established.
    pub fn dns_vsock_fd(&self) -> std::result::Result<RawFd, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(VmCommand::DnsVsockFd { reply: tx }, rx)?
    }

    /// Wait for the guest to send the READY signal on the given vsock port.
    ///
    /// Blocks until `do_wait_for_ready` returns (max ~6 s) or an error occurs.
    /// This is the blocking primitive `Guest::wait_for_ready()` delegates to.
    pub fn wait_for_ready(&self, ready_vsock_port: u32) -> std::result::Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(
            VmCommand::WaitForGuestReady {
                ready_vsock_port,
                reply: tx,
            },
            rx,
        )?
    }

    pub fn add_port_map(
        &self,
        host_port: u16,
        container_port: u16,
    ) -> std::result::Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(
            VmCommand::AddPortMap {
                host_port,
                container_port,
                reply: tx,
            },
            rx,
        )?
    }

    pub fn join(&mut self) -> std::result::Result<(), Error> {
        self.sender
            .blocking_send(VmCommand::Shutdown)
            .map_err(|_| Error::ChannelError("vm thread channel closed".into()))?;

        if let Some(handle) = self.thread.take() {
            handle.join().map_err(|_| Error::ThreadJoin)?;
        }
        Ok(())
    }
}

impl Drop for VmThread {
    fn drop(&mut self) {
        let _ = self.sender.blocking_send(VmCommand::Shutdown);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}
