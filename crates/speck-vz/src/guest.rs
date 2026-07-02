use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use crate::config::{GuestConfig, PortMapConfig};
use crate::error::Error;
use crate::vm_thread::{VmCommand, VmThread};
use crate::vsock::VzSocket;
use speck_core::EventSink;

pub struct Guest {
    thread: VmThread,
    config: GuestConfig,
    sink: Option<Box<dyn EventSink>>,
}

impl Guest {
    pub fn new(config: GuestConfig) -> Self {
        Self {
            thread: VmThread::spawn(),
            config,
            sink: None,
        }
    }

    pub fn with_sink(mut self, sink: Box<dyn EventSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn start(&self) -> Result<speck_core::VmState, Error> {
        let state = self.thread.start(self.config.clone())?;
        let core_state = speck_core::VmState::from(state);
        if let Some(ref sink) = self.sink {
            sink.emit(speck_core::EngineEvent::VmStateChanged {
                state: core_state.clone(),
            });
        }
        Ok(core_state)
    }

    pub fn stop(&self) -> Result<speck_core::VmState, Error> {
        let state = self.thread.stop(self.config.stop_timeout)?;
        let core_state = speck_core::VmState::from(state);
        if let Some(ref sink) = self.sink {
            sink.emit(speck_core::EngineEvent::VmStateChanged {
                state: core_state.clone(),
            });
        }
        Ok(core_state)
    }

    pub fn state(&self) -> speck_core::VmState {
        self.thread
            .state()
            .map(speck_core::VmState::from)
            .unwrap_or(speck_core::VmState::Stopped)
    }

    pub fn vsock_connect(&self, port: u32) -> Result<VzSocket, Error> {
        self.thread.vsock_connect(port)
    }

    pub fn join(&mut self) -> Result<(), Error> {
        self.thread.join()
    }

    pub fn netstack_fd(&self) -> Result<std::os::unix::io::RawFd, Error> {
        self.thread.netstack_fd()
    }

    pub fn dns_vsock_fd(&self) -> Result<std::os::unix::io::RawFd, Error> {
        self.thread.dns_vsock_fd()
    }

    /// Connect to the guest vsock DNS forwarder on `port` and return the raw fd.
    ///
    /// Must be called AFTER `wait_for_ready`. Blocks up to 10 s for the connection.
    pub fn connect_dns_vsock(&self, port: u32) -> Result<std::os::unix::io::RawFd, Error> {
        self.thread.connect_dns_vsock(port)
    }

    pub fn add_port_map(&self, host_port: u16, container_port: u16) -> Result<(), Error> {
        self.thread.add_port_map(host_port, container_port)
    }

    /// Update the pre-provisioned `virtiofs-binds` VirtioFS device with Docker
    /// bind mounts (D-05).
    ///
    /// Bind mounts are accumulated across containers: repeated calls append to the
    /// list and rebuild `VZMultipleDirectoryShare` on the running VM so every
    /// container's bind-mounted host paths remain visible.  Returns
    /// `Err(NotRunning)` if the VM is not yet started.
    pub fn add_bind_mounts(
        &self,
        binds: Vec<crate::config::VolumeMountConfig>,
    ) -> Result<(), Error> {
        self.thread.update_bind_mounts(binds)
    }

    /// Register the netstack port-map sender with the VM thread.
    ///
    /// Delegates to [`VmThread::set_port_map_channel`]. Required by `up.rs` (Plan 06.1-02)
    /// to wire the netstack's receiver so that subsequent `add_port_map` calls are forwarded.
    pub fn set_port_map_channel(
        &self,
        tx: tokio::sync::mpsc::Sender<PortMapConfig>,
    ) -> Result<(), Error> {
        self.thread.set_port_map_channel(tx)
    }

    /// Build a closure that creates a new vsock connection to `port` on demand.
    ///
    /// The returned closure captures a cloned `mpsc::Sender<VmCommand>` from a `std::sync::mpsc`
    /// channel, so it is safe to call from any thread context — `std::thread::spawn` OS threads,
    /// async tasks, or synchronous callers.
    fn vsock_connector_for_port(
        &self,
        port: u32,
    ) -> impl Fn() -> Result<VzSocket, Error> + Send + Sync + 'static {
        let sender = self.thread.clone_cmd_sender();
        move || {
            let (tx, rx) = std::sync::mpsc::channel();
            sender
                .send(VmCommand::VsockConnect { port, reply: tx })
                .map_err(|_| Error::ChannelError("vm thread channel closed".into()))?;
            rx.recv()
                .map_err(|_| Error::ChannelError("vm thread reply channel closed".into()))?
        }
    }

    /// Block until vminitd sends the READY signal on the configured vsock port.
    ///
    /// Delegates to [`VmThread::wait_for_ready`] using `ready_vsock_port` from the config.
    /// Returns an error if `ready_vsock_port` is not set in the config.
    pub fn wait_for_ready(&self) -> Result<(), Error> {
        let ready_vsock_port = self.config.ready_vsock_port.ok_or_else(|| {
            Error::VsockConnect("ready_vsock_port not configured in GuestConfig".into())
        })?;
        self.thread.wait_for_ready(ready_vsock_port)
    }

    /// Connect the in-guest BuildKit gRPC socket to a persistent Unix socket on the host.
    ///
    /// Binds a temporary Unix socket at `/tmp/speck-buildkitd-<pid>-<port>.sock` and spawns
    /// a persistent proxy thread that accepts multiple sequential Unix clients, creating a
    /// fresh vsock connection for each one. Returns the path to the Unix socket.
    pub fn buildkitd_unix_proxy(&self) -> Result<PathBuf, Error> {
        let buildkitd_vsock_port = self
            .config
            .buildkitd_vsock_port
            .ok_or_else(|| Error::VsockConnect("buildkitd_vsock_port not configured".into()))?;

        let sock_path = std::env::temp_dir().join(format!(
            "speck-buildkitd-{}-{}.sock",
            std::process::id(),
            buildkitd_vsock_port,
        ));

        unix_vsock_proxy(
            self.vsock_connector_for_port(buildkitd_vsock_port),
            sock_path.clone(),
            "buildkitd",
        )?;

        Ok(sock_path)
    }

    /// Connect the in-guest containerd gRPC socket to a persistent Unix socket on the host.
    ///
    /// Binds a temporary Unix socket at `/tmp/speck-containerd-<pid>-<port>.sock` and spawns
    /// a persistent proxy thread that accepts multiple sequential Unix clients, creating a
    /// fresh vsock connection for each one. Returns the path to the Unix socket.
    /// Callers can pass that path to `containerd_client::connect()`.
    pub fn containerd_unix_proxy(&self) -> Result<PathBuf, Error> {
        let containerd_vsock_port = self
            .config
            .containerd_vsock_port
            .ok_or_else(|| Error::VsockConnect("containerd_vsock_port not configured".into()))?;

        let sock_path = std::env::temp_dir().join(format!(
            "speck-containerd-{}-{}.sock",
            std::process::id(),
            containerd_vsock_port,
        ));

        unix_vsock_proxy(
            self.vsock_connector_for_port(containerd_vsock_port),
            sock_path.clone(),
            "containerd",
        )?;

        Ok(sock_path)
    }

    /// Expose the guest Docker Engine (moby) socket on a host Unix socket.
    ///
    /// Binds `sock_path` (typically `$SPECK_HOME/speck.sock`) and transparently
    /// forwards every connection to the dockerd vsock port inside the guest.
    /// Because moby is the Docker API reference implementation, no shim layer
    /// is needed — raw bytes flow straight through.
    pub fn docker_api_unix_proxy(&self, sock_path: PathBuf) -> Result<PathBuf, Error> {
        let docker_vsock_port = self.config.docker_vsock_port.unwrap_or(9003);

        unix_vsock_proxy(
            self.vsock_connector_for_port(docker_vsock_port),
            sock_path.clone(),
            "dockerd",
        )?;

        Ok(sock_path)
    }
}

/// Bind a Unix socket at `sock_path` and spawn a persistent proxy thread.
///
/// For each accepted Unix client a fresh vsock connection is created via `connector`, then
/// [`bridge_vsock_unix`] copies bytes bidirectionally. Stale socket files are removed before
/// binding. The proxy thread runs until the listener returns an error.
fn unix_vsock_proxy(
    connector: impl Fn() -> Result<VzSocket, Error> + Send + 'static,
    sock_path: PathBuf,
    label: &'static str,
) -> Result<(), Error> {
    // Remove stale socket file if it exists (e.g., from a previous run).
    let _ = std::fs::remove_file(&sock_path);

    let listener =
        std::os::unix::net::UnixListener::bind(&sock_path).map_err(Error::NetworkIo)?;

    // Persistent proxy thread: accepts multiple clients, each gets its own vsock connection.
    std::thread::spawn(move || loop {
        match listener.accept() {
            Ok((stream, _)) => match connector() {
                Ok(vsock) => {
                    std::thread::spawn(move || bridge_vsock_unix(vsock, stream));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "vsock connect failed for {label} proxy client");
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "{label} unix listener accept error; proxy exiting");
                break;
            }
        }
    });

    Ok(())
}


/// Bidirectional byte bridge between a [`VzSocket`] (vsock) and a [`UnixStream`].
///
/// Each direction runs in its own thread. A `pipe()` is used as a cancellation
/// signal: whichever direction finishes first writes to the pipe; the other
/// direction's `poll()` wakes up and exits. This is necessary because macOS
/// `AF_UNIX` sockets do not unblock a concurrent `read()` via `shutdown()`.
fn bridge_vsock_unix(vsock: VzSocket, stream: std::os::unix::net::UnixStream) {
    use std::io::Write;

    // Dup the vsock fd so both directions have independent ownership.
    let dup_fd = unsafe { libc::dup(vsock.as_raw_fd()) };
    if dup_fd < 0 {
        return;
    }
    let vsock_dup = unsafe { VzSocket::from_raw_fd(dup_fd) };

    let stream_clone = stream.try_clone().expect("clone unix stream");

    // Cancellation pipe: the vsock→unix thread writes a byte when it finishes;
    // the unix→vsock poll() wakes up and exits cleanly.
    let mut pipe_fds = [0i32; 2];
    if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } < 0 {
        return;
    }
    let cancel_r = pipe_fds[0];
    let cancel_w = pipe_fds[1];

    // vsock → unix
    std::thread::spawn(move || {
        let mut stream_write = stream_clone;
        let vsock_read = vsock;
        let mut buf = [0u8; 65536];
        loop {
            match vsock_read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stream_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = stream_write.shutdown(std::net::Shutdown::Write);
        unsafe {
            libc::write(cancel_w, b"\0".as_ptr() as *const libc::c_void, 1);
            libc::close(cancel_w);
        }
    });

    // unix → vsock: poll-based so the cancellation pipe can interrupt it.
    {
        let stream_fd = stream.as_raw_fd();
        let vsock_write = vsock_dup;
        unsafe {
            let flags = libc::fcntl(stream_fd, libc::F_GETFL, 0);
            libc::fcntl(stream_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let mut buf = [0u8; 65536];
        'outer: loop {
            let mut fds = [
                libc::pollfd { fd: stream_fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: cancel_r,  events: libc::POLLIN, revents: 0 },
            ];
            let ret = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if ret <= 0 { break; }

            if fds[1].revents & libc::POLLIN != 0 {
                break;
            }

            if fds[0].revents != 0 {
                loop {
                    let n = unsafe {
                        libc::read(stream_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
                    };
                    if n <= 0 {
                        if n < 0 {
                            let e = std::io::Error::last_os_error();
                            if e.kind() == std::io::ErrorKind::WouldBlock { break; }
                        }
                        break 'outer;
                    }
                    let data = &buf[..n as usize];
                    let mut written = 0;
                    while written < data.len() {
                        match vsock_write.write(&data[written..]) {
                            Ok(0) | Err(_) => break 'outer,
                            Ok(w) => written += w,
                        }
                    }
                }
            }
        }
        unsafe { libc::shutdown(vsock_write.as_raw_fd(), libc::SHUT_RDWR) };
        unsafe { libc::close(cancel_r) };
        drop(stream);
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        let _ = self.thread.send_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speck_core::VmState;
    use std::time::Duration;

    #[test]
    #[ignore = "requires com.apple.security.virtualization entitlement + signed binary"]
    fn test_guest_boots_to_running() {
        let config = GuestConfig {
            kernel_path: std::path::PathBuf::from(
                std::env::var("SPECK_HOME")
                    .map(|h| format!("{h}/kernel/vmlinux"))
                    .unwrap_or_else(|_| {
                        format!(
                            "{}/.local/share/speck/kernel/vmlinux",
                            std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
                        )
                    }),
            ),
            cmdline: "console=hvc0 quiet panic=-1".into(),
            ..Default::default()
        };
        let guest = Guest::new(config);
        let result = guest.start();
        assert!(result.is_ok(), "VM should boot: {:?}", result);
        assert_eq!(result.unwrap(), VmState::Running);
    }

    #[test]
    fn ten_start_stop_cycles() {
        if std::env::var("SPECK_STRESS_TEST").is_err() {
            eprintln!(
                "Skipping: set SPECK_STRESS_TEST=1 to run \
                 (requires virtualization entitlement)"
            );
            return;
        }

        let kernel_path = std::env::var("SPECK_HOME")
            .map(|h| format!("{h}/kernel/vmlinux"))
            .unwrap_or_else(|_| {
                format!(
                    "{}/.local/share/speck/kernel/vmlinux",
                    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
                )
            });
        let config = GuestConfig {
            kernel_path: kernel_path.into(),
            cmdline: "console=hvc0 quiet panic=-1".into(),
            stop_timeout: Duration::from_secs(10),
            ..Default::default()
        };

        for i in 0..10 {
            let guest = Guest::new(config.clone());
            let start = std::time::Instant::now();

            let state = guest
                .start()
                .unwrap_or_else(|e| panic!("Cycle {i}: start should succeed: {e}"));

            assert_eq!(
                state,
                VmState::Running,
                "Cycle {i}: state should be Running"
            );
            let boot_time = start.elapsed();
            println!("Cycle {i}: booted in {boot_time:?}");

            std::thread::sleep(std::time::Duration::from_millis(100));

            guest
                .stop()
                .unwrap_or_else(|e| panic!("Cycle {i}: stop should succeed: {e}"));

            let stop_time = start.elapsed() - boot_time;
            println!("Cycle {i}: stopped in ~{stop_time:?}");
        }
    }
}
