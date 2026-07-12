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

    /// Return the configured vminitd log relay vsock port, if enabled.
    pub fn log_relay_vsock_port(&self) -> Option<u32> {
        self.config.log_relay_vsock_port
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

    let listener = std::os::unix::net::UnixListener::bind(&sock_path).map_err(Error::NetworkIo)?;

    // Persistent proxy thread: accepts multiple clients, each gets its own vsock connection.
    std::thread::spawn(move || {
        loop {
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
        }
    });

    Ok(())
}

/// Bidirectional byte bridge between a [`VzSocket`] (vsock) and a [`UnixStream`].
///
/// Three threads cooperate:
///
/// - a **drain thread** owns the vsock read fd and does nothing but read and
///   queue chunks — its only blocking point is the vsock `read` itself, so
///   Virtualization.framework's fixed 8KB socketpair send buffer can never
///   fill due to downstream backpressure (which makes the framework DROP data
///   and close the connection);
/// - a **downstream writer thread** dequeues chunks and writes them to the
///   unix stream, emitting the half-close and cancel byte only after the
///   queue is fully flushed;
/// - the **calling thread** runs the unix→vsock `poll()` loop.
///
/// A `pipe()` is used as a cancellation signal: the downstream writer thread
/// writes a byte when the vsock→unix direction finishes; the unix→vsock
/// `poll()` wakes up and exits. This is necessary because macOS `AF_UNIX`
/// sockets do not unblock a concurrent `read()` via `shutdown()`.
fn bridge_vsock_unix(vsock: VzSocket, stream: std::os::unix::net::UnixStream) {
    // Dup the vsock fd so both directions have independent ownership.
    let dup_fd = unsafe { libc::dup(vsock.as_raw_fd()) };
    if dup_fd < 0 {
        return;
    }
    let vsock_dup = unsafe { VzSocket::from_raw_fd(dup_fd) };

    let stream_clone = match stream.try_clone() {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!(error = %e, "failed to clone unix stream for vsock bridge; dropping connection");
            return;
        }
    };

    // Cancellation pipe: the vsock→unix thread writes a byte when it finishes;
    // the unix→vsock poll() wakes up and exits cleanly.
    let mut pipe_fds = [0i32; 2];
    if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } < 0 {
        return;
    }
    let cancel_r = pipe_fds[0];
    let cancel_w = pipe_fds[1];

    // vsock → unix: decoupled into a drain thread and a downstream writer
    // thread. An unbounded queue is safe here: vsock connections carry
    // request/response gRPC streams (containerd/BuildKit/dockerd), not
    // infinite firehoses, so the queue depth is bounded in practice by one
    // connection's response burst — the same trade-off as the reorigin
    // channel design. A bounded channel with a blocking send would
    // reintroduce exactly the backpressure that makes Virtualization.framework
    // drop data and close the connection once its fixed 8KB socketpair SNDBUF
    // fills.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();

    // Drain thread: owns the vsock read fd. Never blocks on anything except
    // the vsock read itself, so the framework's writer-side buffer is always
    // drained promptly. On EOF/error it exits, dropping `tx` — the flush
    // signal for the writer thread. The unix→vsock loop's SHUT_RDWR on the
    // dup'd fd (same open file description) unblocks a `read` parked here, so
    // this thread cannot leak when the downstream closes first.
    std::thread::spawn(move || {
        let vsock_read = vsock;
        let mut buf = [0u8; 65536];
        loop {
            match vsock_read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Downstream writer thread: owns the unix write half and `cancel_w`.
    // `recv()` keeps yielding queued chunks after the drain thread drops `tx`
    // and only then disconnects, so the loop naturally flushes the entire
    // remaining queue before teardown. The half-close and cancel byte fire
    // strictly after all guest data has been delivered downstream.
    //
    // NOTE: the unix→vsock loop below sets O_NONBLOCK on the shared open file
    // description, so writes here can fail with WouldBlock under downstream
    // backpressure; `write_chunk_blocking` waits for writability via
    // poll(POLLOUT) instead of erroring out.
    std::thread::spawn(move || {
        let stream_write = stream_clone;
        while let Ok(chunk) = rx.recv() {
            if write_chunk_blocking(&stream_write, &chunk).is_err() {
                break;
            }
        }
        let _ = stream_write.shutdown(std::net::Shutdown::Write);
        unsafe {
            libc::write(cancel_w, c"".as_ptr() as *const libc::c_void, 1);
            libc::close(cancel_w);
        }
    });

    // unix → vsock: poll-based so the cancellation pipe can interrupt it.
    {
        let stream_fd = stream.as_raw_fd();
        let vsock_write = vsock_dup;
        unsafe {
            let flags = libc::fcntl(stream_fd, libc::F_GETFL, 0);
            if flags < 0 || libc::fcntl(stream_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                tracing::warn!(error = %std::io::Error::last_os_error(), "failed to set unix stream nonblocking; dropping connection");
                // `cancel_w` is owned by the downstream writer thread, which
                // writes and closes it exactly once when the vsock→unix
                // direction ends; one-directional bridging continues.
                libc::close(cancel_r);
                return;
            }
        }
        let mut buf = [0u8; 65536];
        'outer: loop {
            let mut fds = [
                libc::pollfd {
                    fd: stream_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: cancel_r,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let ret = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if ret <= 0 {
                break;
            }

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
                            if e.kind() == std::io::ErrorKind::WouldBlock {
                                break;
                            }
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

/// Write `data` fully to `stream`, waiting for writability on `WouldBlock`.
///
/// The unix→vsock poll loop sets `O_NONBLOCK` on the stream's open file
/// description (shared with the `try_clone`d write half), so a stalled
/// downstream consumer surfaces as `WouldBlock` here rather than a blocking
/// write. Waiting on poll(POLLOUT) keeps the writer thread parked without
/// erroring out — the drain thread keeps emptying the vsock fd meanwhile.
fn write_chunk_blocking(
    stream: &std::os::unix::net::UnixStream,
    mut data: &[u8],
) -> std::io::Result<()> {
    use std::io::Write;

    let mut stream_ref = stream;
    while !data.is_empty() {
        match stream_ref.write(data) {
            Ok(0) => return Err(std::io::Error::other("downstream write returned 0")),
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let mut pfd = libc::pollfd {
                    fd: stream.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let ret = unsafe { libc::poll(&mut pfd, 1, -1) };
                if ret < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() != std::io::ErrorKind::Interrupted {
                        return Err(e);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
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

    /// Regression test for guest-to-host vsock data loss under downstream
    /// backpressure.
    ///
    /// `VZVirtioSocketConnection`'s guest-to-host path flows through a
    /// framework-internal unix socketpair whose writer-side SO_SNDBUF is fixed
    /// at 8192 bytes. If the bridge blocks on downstream unix writes, that 8KB
    /// fills and the framework drops the remaining data and closes the
    /// connection. This test mimics the framework side with a socketpair whose
    /// writer SNDBUF is capped at 8192 and a downstream client that reads
    /// NOTHING for the entire duration of a 64KB write: the bridge must drain
    /// the vsock fd continuously regardless of downstream backpressure.
    #[test]
    fn bridge_drains_vsock_with_stalled_downstream() {
        use std::io::{Read, Write};
        use std::os::unix::io::IntoRawFd;
        use std::os::unix::net::UnixStream;

        // Fake Virtualization.framework socketpair. Cap the WRITER side send
        // buffer at 8192 to mimic the framework's fixed internal buffer (on
        // macOS AF_UNIX, in-flight capacity is governed solely by the writer's
        // SNDBUF).
        let (mut fw_writer, vsock_end) = UnixStream::pair().expect("framework socketpair");
        let sndbuf: libc::c_int = 8192;
        let rc = unsafe {
            libc::setsockopt(
                fw_writer.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &sndbuf as *const libc::c_int as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        assert_eq!(rc, 0, "setsockopt(SO_SNDBUF=8192) failed");
        let vsock = unsafe { VzSocket::from_raw_fd(vsock_end.into_raw_fd()) };

        // Downstream pair: bridge_end goes into the bridge; client_end stalls.
        let (bridge_end, mut client_end) = UnixStream::pair().expect("downstream socketpair");

        // bridge_vsock_unix blocks its caller in the unix→vsock poll loop, so
        // run it on its own thread.
        let bridge = std::thread::spawn(move || bridge_vsock_unix(vsock, bridge_end));

        const TOTAL: usize = 65536;
        let expected: Vec<u8> = (0..TOTAL).map(|i| (i % 251) as u8).collect();

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let payload = expected.clone();
        let writer = std::thread::spawn(move || {
            fw_writer
                .write_all(&payload)
                .expect("framework writer failed");
            drop(fw_writer); // EOF for the drain side
            let _ = done_tx.send(());
        });

        // CRITICAL: nothing reads from client_end before this assertion — the
        // downstream stall during the entire write is the point of the test.
        done_rx.recv_timeout(Duration::from_secs(5)).expect(
            "vsock bridge applied downstream backpressure to the framework's \
             8KB buffer: the 64KB guest-side write did not complete within 5s",
        );
        writer.join().expect("framework writer thread panicked");

        // Only now does the stalled client read. Everything must arrive
        // intact, followed by EOF — proving the half-close and cancel byte
        // fired only after the queue was fully flushed.
        let mut got = Vec::new();
        client_end
            .read_to_end(&mut got)
            .expect("downstream read_to_end failed");
        assert_eq!(got.len(), TOTAL, "byte count mismatch at downstream");
        assert_eq!(got, expected, "payload corrupted in transit");
        drop(client_end);

        // The bridge call itself must return: the cancel pipe still
        // terminates the unix→vsock poll loop.
        bridge.join().expect("bridge thread panicked");
    }

    /// Regression test for half-close semantics on the unix→vsock direction.
    ///
    /// A docker CLI half-close (`CloseWrite` after stdin EOF on `exec -i` /
    /// hijacked streams) surfaces as read-EOF on the unix side of the bridge.
    /// That must shut down ONLY the vsock write direction (`SHUT_WR`): the
    /// guest may still produce output afterwards, and those bytes must reach
    /// the client. A `SHUT_RDWR` here kills the drain thread's read (the dup
    /// shares the same open file description) and loses everything the guest
    /// writes after stdin EOF.
    #[test]
    fn bridge_half_close_delivers_data_after_unix_closewrite() {
        use std::io::{Read, Write};
        use std::os::unix::io::IntoRawFd;
        use std::os::unix::net::UnixStream;

        // Fake vsock: guest_end plays the guest, vsock_end goes into the bridge.
        let (mut guest_end, vsock_end) = UnixStream::pair().expect("fake vsock socketpair");
        let vsock = unsafe { VzSocket::from_raw_fd(vsock_end.into_raw_fd()) };

        // Downstream pair: bridge_end goes into the bridge; client_end is the
        // docker-CLI-like client.
        let (bridge_end, mut client_end) = UnixStream::pair().expect("downstream socketpair");

        let (bridge_done_tx, bridge_done_rx) = std::sync::mpsc::channel();
        let bridge = std::thread::spawn(move || {
            bridge_vsock_unix(vsock, bridge_end);
            let _ = bridge_done_tx.send(());
        });

        // 1. Client sends its request.
        client_end
            .write_all(b"REQUEST")
            .expect("client request write failed");

        // 2. Guest reads exactly the request bytes.
        let mut req = [0u8; 7];
        guest_end
            .read_exact(&mut req)
            .expect("guest did not receive the request");
        assert_eq!(&req, b"REQUEST");

        // 3. Client half-closes (docker CLI CloseWrite / stdin EOF).
        client_end
            .shutdown(std::net::Shutdown::Write)
            .expect("client CloseWrite failed");

        // 4. Guest observes EOF on its read side — the half-close propagated
        //    as SHUT_WR on the vsock write direction.
        let mut sink = [0u8; 16];
        loop {
            match guest_end.read(&mut sink) {
                Ok(0) => break,
                Ok(_) => panic!("unexpected extra bytes on guest read side"),
                Err(e) => panic!("guest read failed while waiting for EOF: {e}"),
            }
        }

        // 5. Guest THEN produces output — this is the data a SHUT_RDWR
        //    teardown would lose (or reject with EPIPE).
        guest_end
            .write_all(b"RESPONSE")
            .expect("guest write after client CloseWrite failed");
        drop(guest_end); // EOF for the drain thread

        // 6. All post-half-close output must reach the client, then EOF.
        let mut got = Vec::new();
        client_end
            .read_to_end(&mut got)
            .expect("client read_to_end failed");
        assert_eq!(got, b"RESPONSE", "guest output after CloseWrite was lost");

        // 7. The bridge must tear down fully (cancel byte from the drained
        //    vsock→unix direction releases the parked unix→vsock thread).
        bridge_done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("bridge did not tear down within 5s after guest EOF");
        bridge.join().expect("bridge thread panicked");
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
