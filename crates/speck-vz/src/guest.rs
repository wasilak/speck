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

    /// Expose guest Podman's Docker-compatible API on a host Unix socket.
    ///
    /// Binds exactly `sock_path` (typically `$SPECK_HOME/speck.sock`) and spawns a persistent
    /// proxy thread that accepts multiple sequential Unix clients, forwarding each one to the
    /// Podman vsock port inside the guest.
    ///
    /// Uses `self.config.podman_vsock_port` when set, falling back to the spike default `9003`.
    ///
    /// Returns the socket path that was bound (same as `sock_path`).
    pub fn docker_api_unix_proxy(&self, sock_path: PathBuf) -> Result<PathBuf, Error> {
        let podman_vsock_port = self.config.podman_vsock_port.unwrap_or(9003);

        unix_vsock_docker_proxy(self.vsock_connector_for_port(podman_vsock_port), sock_path.clone())?;

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

/// Bind a Unix socket at `sock_path` and spawn a Docker-API-aware proxy thread.
///
/// Like `unix_vsock_proxy` but intercepts `POST /containers/{id}/attach` requests and
/// pre-starts the container first. Podman v5 compat API returns 409 if you attach to a
/// container in "created" state; Docker Engine allows pre-attach. This proxy bridges the gap.
fn unix_vsock_docker_proxy(
    connector: impl Fn() -> Result<VzSocket, Error> + Send + Sync + 'static,
    sock_path: PathBuf,
) -> Result<(), Error> {
    use std::sync::Arc;

    let _ = std::fs::remove_file(&sock_path);
    let listener =
        std::os::unix::net::UnixListener::bind(&sock_path).map_err(Error::NetworkIo)?;

    let connector = Arc::new(connector);
    let wait_cache: WaitCodeCache = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    std::thread::spawn(move || loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let connector = Arc::clone(&connector);
                let cache = Arc::clone(&wait_cache);
                std::thread::spawn(move || {
                    docker_proxy_bridge(move || connector(), stream, cache)
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "docker-api unix listener accept error; proxy exiting");
                break;
            }
        }
    });

    Ok(())
}

/// Shared cache: container_id → exit StatusCode captured from a pre-emptive /wait.
/// Used to answer docker CLI's POST /containers/{id}/wait when the container was
/// already auto-removed (Podman returns 404) before docker CLI's goroutine arrived.
type WaitCodeCache = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, i64>>>;

/// Docker API-aware bridge.
///
/// Intercepts two request types:
///
/// **POST /containers/{id}/attach** — Podman v5 returns 409 on pre-start attach.
///   1. Open a pre-emptive POST /wait on a separate vsock (background thread). This
///      blocks until the container exits and caches the exit StatusCode; it runs
///      concurrently so it doesn't delay the attach path.
///   2. Fire-and-forget POST /start.
///   3. Open GET /logs?follow=true (HTTP/1.0 to avoid chunked encoding).
///   4. Send synthetic 101 to docker CLI.
///   5. Stream logs body (Docker multiplex format) to docker CLI.
///
/// **POST /containers/{id}/wait** — Podman returns 404 after AutoRemove.
///   If Podman returns 404 we check the WaitCodeCache. If found, reply with the
///   cached StatusCode so docker CLI exits with the correct code. Retry up to
///   100 ms while the background /wait thread populates the cache.
///
/// All other requests: normal vsock byte bridge.
fn docker_proxy_bridge(
    connector: impl Fn() -> Result<VzSocket, Error> + Send + 'static,
    stream: std::os::unix::net::UnixStream,
    wait_cache: WaitCodeCache,
) {
    use std::io::{Read, Write};

    // Read HTTP request headers byte-by-byte until \r\n\r\n.
    let mut req_headers: Vec<u8> = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        match (&stream).read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                req_headers.push(byte[0]);
                let len = req_headers.len();
                if len >= 4 && &req_headers[len - 4..] == b"\r\n\r\n" {
                    break;
                }
                if len > 16384 {
                    break;
                }
            }
        }
    }

    // --- /wait intercept ---
    //
    // Don't forward /wait to Podman: Podman may already have AutoRemoved the
    // container (→ 404), or it may not close an HTTP/1.0 connection cleanly
    // for this endpoint. Instead we rely on the background /wait thread spawned
    // inside the /attach handler, which fires as soon as the container exits and
    // stores the exit code in wait_cache. We poll here (up to 60 s) and return
    // the cached StatusCode to docker CLI once it's available.
    if let Some(wait_cid) = detect_docker_op(&req_headers, "/wait") {
        let mut code: Option<i64> = None;
        for _ in 0..6000 {
            code = wait_cache.lock().unwrap().get(&wait_cid).copied();
            if code.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let code = code.unwrap_or(0);
        wait_cache.lock().unwrap().remove(&wait_cid);

        // DELETE the container — AutoRemove was stripped from /create so Podman won't
        // clean it up automatically. This runs in the background so it doesn't delay
        // the response to docker CLI.
        let del_cid = wait_cid.clone();
        if let Ok(del_vsock) = connector() {
            std::thread::spawn(move || {
                let req = format!(
                    "DELETE /containers/{del_cid}?force=true HTTP/1.0\r\nHost: localhost\r\n\r\n"
                );
                vsock_write_all(&del_vsock, req.as_bytes());
                // Drain until EOF or end-of-chunks so Podman has time to complete.
                let mut buf = [0u8; 1024];
                let mut resp: Vec<u8> = Vec::new();
                loop {
                    match del_vsock.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            resp.extend_from_slice(&buf[..n]);
                            if resp.ends_with(b"0\r\n\r\n") || resp.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
            });
        }

        let body = format!("{{\"StatusCode\":{code}}}");
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = (&stream).write_all(resp.as_bytes());
        return;
    }

    // --- /start intercept ---
    //
    // We already started the container inside the /attach handler via
    // vsock_request_status. Forwarding docker CLI's /start to Podman would re-run
    // the container (it's now in "exited" state because AutoRemove was stripped).
    // Return a synthetic 204 so docker CLI considers the start successful.
    if detect_docker_op(&req_headers, "/start").is_some() {
        let resp = b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
        let _ = (&stream).write_all(resp);
        return;
    }

    // --- /resize intercept ---
    //
    // docker CLI sends POST /containers/{id}/resize right after /start returns.
    // Because our /start intercept is synthetic (the real start happens in the
    // /attach handler), the container may still be in "created" state when /resize
    // arrives at Podman.  Retry until Podman accepts it (container is running) or
    // give up after ~1 s and return success anyway — wrong size is non-fatal.
    if detect_docker_op(&req_headers, "/resize").is_some() {
        for attempt in 0..10u8 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let vsock = match connector() {
                Ok(v) => v,
                Err(_) => break,
            };
            vsock_write_all(&vsock, &req_headers);
            let mut resp_hdrs: Vec<u8> = Vec::with_capacity(256);
            loop {
                match vsock.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        resp_hdrs.push(byte[0]);
                        let len = resp_hdrs.len();
                        if len >= 4 && &resp_hdrs[len - 4..] == b"\r\n\r\n" { break; }
                        if len > 8192 { break; }
                    }
                }
            }
            let status = http_status_code(&resp_hdrs);
            if status == 200 || status == 204 {
                let out = inject_connection_close(&resp_hdrs);
                let _ = (&stream).write_all(&out);
                return;
            }
        }
        // Timed out — return 200 so docker CLI doesn't print the warning.
        let _ = (&stream).write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }

    // --- /attach intercept ---
    if let Some(container_id) = detect_docker_op(&req_headers, "/attach") {
        // docker CLI passes `stdin=1` for `docker run -i` / `docker run -ti`.
        // Interactive containers need a direct /attach proxy (bidirectional, TTY-capable).
        // Non-interactive containers use /logs?tail=all so output from fast-exiting
        // containers (e.g. `echo`) isn't lost when the stream arrives after the container exits.
        let req_line = std::str::from_utf8(&req_headers).unwrap_or("");
        let wants_stdin = req_line.contains("stdin=1");

        if wants_stdin {
            // --- interactive path: start container then forward /attach directly ---
            //
            // Start the container first so it's running when /attach arrives. We then
            // proxy /attach directly to Podman (bidirectional), which supports both
            // piped stdin (-i) and full TTY (-ti).
            let start_status = start_container(&connector, &container_id);
            if start_status != 204 && start_status != 304 && start_status != 200 {
                let body = format!("{{\"message\":\"container start failed (status {start_status})\"}}");
                let err = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = (&stream).write(err.as_bytes());
                return;
            }

            let attach_vsock = match connector() {
                Ok(v) => v,
                Err(_) => return,
            };
            vsock_write_all(&attach_vsock, &req_headers);

            let mut attach_resp: Vec<u8> = Vec::with_capacity(256);
            loop {
                match attach_vsock.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        attach_resp.push(byte[0]);
                        let len = attach_resp.len();
                        if len >= 4 && &attach_resp[len - 4..] == b"\r\n\r\n" { break; }
                        if len > 8192 { break; }
                    }
                }
            }

            let status = http_status_code(&attach_resp);
            if status != 101 {
                let _ = (&stream).write_all(&attach_resp);
                return;
            }
            if (&stream).write_all(&attach_resp).is_err() {
                return;
            }

            // stdin thread: docker CLI → Podman.
            let attach_fd = attach_vsock.as_raw_fd();
            let dup_fd = unsafe { libc::dup(attach_fd) };
            if dup_fd < 0 { return; }
            let attach_write = unsafe { VzSocket::from_raw_fd(dup_fd) };
            let stream_clone = match stream.try_clone() { Ok(c) => c, Err(_) => return };
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match (&stream_clone).read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => { let _ = attach_write.write(&buf[..n]); }
                    }
                }
                unsafe { libc::shutdown(attach_fd, libc::SHUT_WR) };
            });

            // stdout/stderr: Podman → docker CLI.
            let mut buf = [0u8; 4096];
            loop {
                match attach_vsock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if (&stream).write_all(&buf[..n]).is_err() { break; }
                    }
                }
            }
        } else {
            // --- non-interactive path: start container then use /logs ---
            // /logs?tail=all replays historical output, so even fast containers whose
            // stdout was produced before the attach arrived show their output correctly.
            let start_status = start_container(&connector, &container_id);
            if start_status != 204 && start_status != 304 && start_status != 200 {
                let body = format!("{{\"message\":\"container start failed (status {start_status})\"}}");
                let err = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = (&stream).write(err.as_bytes());
                return;
            }

            let logs_vsock = match connector() {
                Ok(v) => v,
                Err(_) => return,
            };
            let logs_req = format!(
                "GET /containers/{container_id}/logs?stdout=1&stderr=1&follow=true&tail=all HTTP/1.0\r\nHost: localhost\r\n\r\n"
            );
            vsock_write_all(&logs_vsock, logs_req.as_bytes());

            let mut logs_resp_headers: Vec<u8> = Vec::with_capacity(256);
            loop {
                match logs_vsock.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        logs_resp_headers.push(byte[0]);
                        let len = logs_resp_headers.len();
                        if len >= 4 && &logs_resp_headers[len - 4..] == b"\r\n\r\n" { break; }
                        if len > 8192 { break; }
                    }
                }
            }

            let status = http_status_code(&logs_resp_headers);
            if status != 200 {
                let err = format!("HTTP/1.1 {status} Error\r\nContent-Length: 0\r\n\r\n");
                let _ = (&stream).write(err.as_bytes());
                return;
            }

            let reply_101 = b"HTTP/1.1 101 UPGRADED\r\nContent-Type: application/vnd.docker.raw-stream\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n";
            if (&stream).write_all(reply_101).is_err() {
                return;
            }

            // Watcher thread: shut down logs read when docker CLI disconnects.
            let logs_vsock_fd = logs_vsock.as_raw_fd();
            let stream_clone = match stream.try_clone() { Ok(c) => c, Err(_) => return };
            std::thread::spawn(move || {
                let mut buf = [0u8; 256];
                loop {
                    match (&stream_clone).read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                unsafe { libc::shutdown(logs_vsock_fd, libc::SHUT_RD) };
            });

            let mut buf = [0u8; 4096];
            loop {
                match logs_vsock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut dst = &stream;
                        let mut pos = 0;
                        while pos < n {
                            match dst.write(&buf[pos..n]) {
                                Ok(0) | Err(_) => return,
                                Ok(k) => pos += k,
                            }
                        }
                    }
                }
            }
        }

        // Logs stream closed: container has exited. Capture exit code via inspect in a
        // background thread so the /attach connection can close immediately. The /wait
        // intercept polls the cache for up to 60 s — plenty of time for inspect to land.
        // Brief sleep: Podman may close the logs stream a few ms before committing
        // the final exit code to its container database.
        {
            let cid = container_id.clone();
            let cache = wait_cache.clone();
            if let Ok(insp_vsock) = connector() {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let req = format!(
                        "GET /containers/{cid}/json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                    );
                    vsock_write_all(&insp_vsock, req.as_bytes());
                    // Read headers byte-by-byte.
                    let mut insp_hdrs: Vec<u8> = Vec::with_capacity(512);
                    let mut insp_b = [0u8; 1];
                    loop {
                        match insp_vsock.read(&mut insp_b) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                insp_hdrs.push(insp_b[0]);
                                let len = insp_hdrs.len();
                                if len >= 4 && &insp_hdrs[len - 4..] == b"\r\n\r\n" { break; }
                                if len > 8192 { break; }
                            }
                        }
                    }
                    if http_status_code(&insp_hdrs) == 200 {
                        let content_length = parse_content_length(&insp_hdrs);
                        let mut insp_body: Vec<u8> = Vec::with_capacity(4096);
                        let mut insp_buf = [0u8; 512];
                        loop {
                            match insp_vsock.read(&mut insp_buf) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    insp_body.extend_from_slice(&insp_buf[..n]);
                                    if insp_body.ends_with(b"0\r\n\r\n") { break; }
                                    if content_length.map_or(false, |cl| insp_body.len() >= cl) { break; }
                                }
                            }
                        }
                        if let Some(code) = parse_inspect_exit_code(&insp_body) {
                            cache.lock().unwrap().insert(cid, code);
                        }
                    }
                });
            }
        }
        return;
    }

    // --- /create intercept (strip AutoRemove) ---
    //
    // With AutoRemove=true, Podman deletes the container immediately after it exits.
    // This creates a race: our background /wait thread may not reach Podman before
    // the container is removed, so the exit code is lost and docker CLI's /start
    // (which arrives after /wait) gets a 404. By stripping AutoRemove we keep the
    // container in "exited" state until our /wait intercept explicitly DELETEs it.
    if is_create_endpoint(&req_headers) {
        proxy_create_request(&connector, stream, &req_headers);
        return;
    }

    // --- normal request: proxy with Connection: close ---
    // Adding Connection: close forces docker CLI to open a fresh connection for every
    // subsequent request (e.g. /create, /wait), so our per-connection intercepts above
    // can see and handle them individually.
    proxy_single_request(&connector, stream, &req_headers);
}

/// Read the HTTP response status code from the first line of `headers`.
fn http_status_code(headers: &[u8]) -> u16 {
    let end = headers.iter().position(|&b| b == b'\n').unwrap_or(headers.len());
    let line = std::str::from_utf8(&headers[..end]).unwrap_or("").trim();
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Forward one HTTP request to Podman, inject `Connection: close` into the response,
/// and return. Because the response says "close", docker CLI opens a fresh connection
/// for every subsequent request — allowing our per-connection intercept logic to see
/// requests like /containers/{id}/wait individually on their own connections, where
/// the /wait arrives at Podman before the container is started and blocks correctly.
fn proxy_single_request(
    connector: &impl Fn() -> Result<VzSocket, Error>,
    stream: std::os::unix::net::UnixStream,
    req_headers: &[u8],
) {
    use std::io::{Read, Write};

    let vsock = match connector() {
        Ok(v) => v,
        Err(_) => return,
    };

    // Read request body if Content-Length > 0.
    let body_len = parse_content_length(req_headers).unwrap_or(0);
    let mut req_body: Vec<u8> = vec![0u8; body_len];
    if body_len > 0 {
        let mut pos = 0;
        while pos < body_len {
            match (&stream).read(&mut req_body[pos..]) {
                Ok(0) | Err(_) => return,
                Ok(k) => pos += k,
            }
        }
    }

    // Forward request to Podman.
    vsock_write_all(&vsock, req_headers);
    if !req_body.is_empty() {
        vsock_write_all(&vsock, &req_body);
    }

    // Read Podman response headers.
    let mut resp_hdrs: Vec<u8> = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        match vsock.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                resp_hdrs.push(byte[0]);
                let len = resp_hdrs.len();
                if len >= 4 && &resp_hdrs[len - 4..] == b"\r\n\r\n" {
                    break;
                }
                if len > 32768 {
                    break;
                }
            }
        }
    }

    // Inject Connection: close into the response.
    let resp_hdrs = inject_connection_close(&resp_hdrs);
    let _ = (&stream).write_all(&resp_hdrs);

    // Forward Podman response body to docker CLI.
    let mut buf = [0u8; 4096];
    loop {
        match vsock.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if (&stream).write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
}

/// Parse the numeric value of a `Content-Length` header, or return 0 if absent.
fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(headers).ok()?;
    for line in s.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

/// Rewrite response headers to include `Connection: close`, stripping any existing
/// `Connection` header.  The trailing `\r\n\r\n` is re-added.
fn inject_connection_close(headers: &[u8]) -> Vec<u8> {
    let s = match std::str::from_utf8(headers) {
        Ok(s) => s,
        Err(_) => return headers.to_vec(),
    };
    // Split on \r\n, drop existing Connection header, then append our own.
    let mut out = String::with_capacity(headers.len() + 32);
    for line in s.split("\r\n") {
        if line.to_ascii_lowercase().starts_with("connection:") {
            continue; // drop
        }
        if line.is_empty() {
            continue; // the trailing empty line — we'll add it back ourselves
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    out.into_bytes()
}

/// Return the container ID if the first request line is `POST /containers/{id}/{op}`.
fn detect_docker_op(headers: &[u8], op: &str) -> Option<String> {
    let first_line_end = headers.iter().position(|&b| b == b'\n')?;
    let first_line = std::str::from_utf8(&headers[..first_line_end]).ok()?.trim();
    let url = first_line.strip_prefix("POST ")?.split_whitespace().next()?;
    let after = url.find("/containers/").map(|i| &url[i + "/containers/".len()..])?;
    let slash = after.find('/')?;
    let id = &after[..slash];
    if id.is_empty() {
        return None;
    }
    if after[slash..].starts_with(op) {
        Some(id.to_string())
    } else {
        None
    }
}

/// POST /containers/{id}/start and return the HTTP status code.
fn start_container(
    connector: &impl Fn() -> Result<VzSocket, Error>,
    container_id: &str,
) -> u16 {
    let vsock = match connector() {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let req = format!(
        "POST /containers/{container_id}/start HTTP/1.0\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n"
    );
    vsock_write_all(&vsock, req.as_bytes());
    let mut hdrs: Vec<u8> = Vec::with_capacity(256);
    let mut b = [0u8; 1];
    loop {
        match vsock.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                hdrs.push(b[0]);
                let len = hdrs.len();
                if len >= 4 && &hdrs[len - 4..] == b"\r\n\r\n" { break; }
                if len > 4096 { break; }
            }
        }
    }
    http_status_code(&hdrs)
}

/// Extract `State.ExitCode` from a Podman container inspect JSON response.
fn parse_inspect_exit_code(resp: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(resp).ok()?;
    let idx = s.find("\"ExitCode\":")?;
    let tail = s[idx + "\"ExitCode\":".len()..].trim_start();
    let end = tail.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(tail.len());
    tail[..end].parse().ok()
}

/// Write all bytes to a VzSocket, returning early on error.
fn vsock_write_all(vsock: &VzSocket, data: &[u8]) {
    let mut pos = 0;
    while pos < data.len() {
        match vsock.write(&data[pos..]) {
            Ok(0) | Err(_) => return,
            Ok(n) => pos += n,
        }
    }
}


/// Return true if the request is `POST .../containers/create`.
fn is_create_endpoint(headers: &[u8]) -> bool {
    let end = headers.iter().position(|&b| b == b'\n').unwrap_or(0);
    let first = std::str::from_utf8(&headers[..end]).unwrap_or("").trim();
    let url = match first.strip_prefix("POST ").and_then(|s| s.split_whitespace().next()) {
        Some(u) => u,
        None => return false,
    };
    url.ends_with("/containers/create")
}

/// Replace `"AutoRemove":true` with `"AutoRemove":false` in a JSON body.
fn strip_auto_remove(body: &[u8]) -> Vec<u8> {
    let s = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return body.to_vec(),
    };
    let s = s.replace("\"AutoRemove\":true", "\"AutoRemove\":false");
    let s = s.replace("\"AutoRemove\": true", "\"AutoRemove\": false");
    s.into_bytes()
}

/// Rewrite the `Content-Length` header value in `headers` to `new_len`.
fn rewrite_content_length(headers: &[u8], new_len: usize) -> Vec<u8> {
    let s = match std::str::from_utf8(headers) {
        Ok(s) => s,
        Err(_) => return headers.to_vec(),
    };
    let mut out = String::with_capacity(headers.len() + 8);
    for line in s.split("\r\n") {
        if line.to_ascii_lowercase().starts_with("content-length:") {
            out.push_str(&format!("Content-Length: {new_len}\r\n"));
        } else if line.is_empty() {
            continue;
        } else {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("\r\n");
    out.into_bytes()
}

/// Like `proxy_single_request` but strips `AutoRemove:true` from the request body
/// so the container stays in "exited" state after it finishes (giving the background
/// /wait thread a chance to capture the exit code before cleanup).
fn proxy_create_request(
    connector: &impl Fn() -> Result<VzSocket, Error>,
    stream: std::os::unix::net::UnixStream,
    req_headers: &[u8],
) {
    use std::io::{Read, Write};

    let body_len = parse_content_length(req_headers).unwrap_or(0);
    let mut req_body: Vec<u8> = vec![0u8; body_len];
    if body_len > 0 {
        let mut pos = 0;
        while pos < body_len {
            match (&stream).read(&mut req_body[pos..]) {
                Ok(0) | Err(_) => return,
                Ok(k) => pos += k,
            }
        }
    }

    let new_body = strip_auto_remove(&req_body);
    let new_headers = rewrite_content_length(req_headers, new_body.len());

    let vsock = match connector() {
        Ok(v) => v,
        Err(_) => return,
    };
    vsock_write_all(&vsock, &new_headers);
    if !new_body.is_empty() {
        vsock_write_all(&vsock, &new_body);
    }

    // Read Podman response headers.
    let mut resp_hdrs: Vec<u8> = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        match vsock.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                resp_hdrs.push(byte[0]);
                let len = resp_hdrs.len();
                if len >= 4 && &resp_hdrs[len - 4..] == b"\r\n\r\n" {
                    break;
                }
                if len > 32768 {
                    break;
                }
            }
        }
    }

    let resp_hdrs = inject_connection_close(&resp_hdrs);
    let _ = (&stream).write_all(&resp_hdrs);

    let mut buf = [0u8; 4096];
    loop {
        match vsock.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if (&stream).write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
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
