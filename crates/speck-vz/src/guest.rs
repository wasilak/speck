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

    pub fn add_port_map(&self, host_port: u16, container_port: u16) -> Result<(), Error> {
        self.thread.add_port_map(host_port, container_port)
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
    /// The returned closure captures a cloned `mpsc::Sender<VmCommand>` and is safe to call
    /// from `std::thread::spawn` OS-thread context. It must NOT be called from an async task
    /// because `blocking_send` / `blocking_recv` will panic inside a tokio executor
    /// (T-06.1-02 threat model — vsock_connector_for_port called from tokio task).
    fn vsock_connector_for_port(
        &self,
        port: u32,
    ) -> impl Fn() -> Result<VzSocket, Error> + Send + 'static {
        let sender = self.thread.clone_cmd_sender();
        move || {
            let (tx, rx) = tokio::sync::oneshot::channel();
            sender
                .blocking_send(VmCommand::VsockConnect { port, reply: tx })
                .map_err(|_| Error::ChannelError("vm thread channel closed".into()))?;
            rx.blocking_recv()
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

        let connector = self.vsock_connector_for_port(buildkitd_vsock_port);

        let sock_path = std::env::temp_dir().join(format!(
            "speck-buildkitd-{}-{}.sock",
            std::process::id(),
            buildkitd_vsock_port,
        ));

        // Remove stale socket file if it exists.
        let _ = std::fs::remove_file(&sock_path);

        let listener =
            std::os::unix::net::UnixListener::bind(&sock_path).map_err(Error::NetworkIo)?;

        // Persistent proxy thread: accepts multiple clients, each gets its own vsock connection.
        std::thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => match connector() {
                        Ok(vsock) => {
                            std::thread::spawn(move || bridge_vsock_unix(vsock, stream));
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "vsock connect failed for buildkitd proxy client"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "buildkitd unix listener accept error; proxy exiting"
                        );
                        break;
                    }
                }
            }
        });

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

        let connector = self.vsock_connector_for_port(containerd_vsock_port);

        let sock_path = std::env::temp_dir().join(format!(
            "speck-containerd-{}-{}.sock",
            std::process::id(),
            containerd_vsock_port,
        ));

        // Remove stale socket file if it exists (e.g., from a previous run).
        let _ = std::fs::remove_file(&sock_path);

        let listener =
            std::os::unix::net::UnixListener::bind(&sock_path).map_err(Error::NetworkIo)?;

        // Persistent proxy thread: accepts multiple clients, each gets its own vsock connection.
        std::thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => match connector() {
                        Ok(vsock) => {
                            std::thread::spawn(move || bridge_vsock_unix(vsock, stream));
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "vsock connect failed for containerd proxy client"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "containerd unix listener accept error; proxy exiting"
                        );
                        break;
                    }
                }
            }
        });

        Ok(sock_path)
    }
}

/// Bidirectional byte bridge between a [`VzSocket`] (vsock) and a [`UnixStream`].
///
/// Spawns one thread for the vsock→unix direction and handles unix→vsock in the current thread.
/// Errors in either direction silently end the copy loop; the fds are closed on drop.
fn bridge_vsock_unix(vsock: VzSocket, stream: std::os::unix::net::UnixStream) {
    use std::io::{Read, Write};

    // Dup the vsock fd so both directions have independent ownership.
    let dup_fd = unsafe { libc::dup(vsock.as_raw_fd()) };
    if dup_fd < 0 {
        return; // dup failed; bridge cannot start
    }
    let vsock_dup = unsafe { VzSocket::from_raw_fd(dup_fd) };

    let stream_clone = stream.try_clone().expect("clone unix stream");

    // vsock → unix (in a separate thread)
    std::thread::spawn(move || {
        let mut stream_write = stream_clone;
        let vsock_read = vsock;
        let mut buf = [0u8; 4096];
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
    });

    // unix → vsock (current thread)
    {
        let mut stream_read = stream;
        let vsock_write = vsock_dup;
        let mut buf = [0u8; 4096];
        loop {
            match stream_read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut written = 0;
                    while written < n {
                        match vsock_write.write(&buf[written..n]) {
                            Ok(0) | Err(_) => return,
                            Ok(w) => written += w,
                        }
                    }
                }
            }
        }
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
