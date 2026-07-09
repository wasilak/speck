//! Guest-side vsock log relay for Docker stdout/stderr FIFOs.
//!
//! `dockerd`/containerd task I/O is configured as FIFO paths at task-create
//! time. The host cannot read those FIFOs over containerd's gRPC API, so this
//! service runs inside `vminitd`, captures FIFO bytes into memory, and serves
//! them to the host over a small line-based vsock protocol.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::ffi::CString;
use std::io;
use std::sync::{Arc, Mutex};

/// Maximum bytes retained per log stream inside the guest.
///
/// When a stream exceeds this ceiling the oldest bytes are dropped and
/// `base_offset` advances, so absolute offset-based reads from the host
/// stay correct: the host asks for offset X and gets `bytes[X - base_offset..]`,
/// or the full retained buffer if X has been trimmed away.
const MAX_LOG_STREAM_BYTES: usize = 32 * 1024 * 1024; // 32 MiB

type LogBuffers = Arc<Mutex<HashMap<String, ContainerLogBuffers>>>;

#[derive(Clone)]
struct ContainerLogBuffers {
    stdout: Arc<Mutex<RetainedLogBuffer>>,
    stderr: Arc<Mutex<RetainedLogBuffer>>,
}

/// A bounded byte buffer that tracks the absolute offset of its first byte.
///
/// When `bytes` exceeds `max_bytes` the oldest bytes are dropped and
/// `base_offset` is advanced accordingly.  Reads with an absolute offset
/// older than the current `base_offset` return the full current buffer;
/// reads with a future offset return empty.
#[derive(Clone)]
struct RetainedLogBuffer {
    /// Absolute offset of `bytes[0]` — the byte position in the
    /// cumulative stream where this buffer starts.
    base_offset: usize,
    /// Retained bytes (at most `max_bytes`).
    bytes: Vec<u8>,
    /// Hard ceiling after which trimming kicks in.
    max_bytes: usize,
}

impl RetainedLogBuffer {
    fn new(max_bytes: usize) -> Self {
        Self {
            base_offset: 0,
            bytes: Vec::new(),
            max_bytes,
        }
    }

    /// Append new data and trim if above the ceiling.
    fn append(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.bytes.extend_from_slice(data);
        self.trim();
    }

    /// Return bytes starting at `absolute_offset`.
    ///
    /// If the requested offset falls before `base_offset`, the entire
    /// retained buffer is returned (that's the best we can do).  If the
    /// requested offset is past the end of the retained bytes, an empty
    /// slice is returned.
    fn read_from(&self, absolute_offset: usize) -> &[u8] {
        if absolute_offset < self.base_offset {
            // Requested data has been trimmed — return whatever we have.
            return &self.bytes;
        }
        let local_offset = absolute_offset - self.base_offset;
        if local_offset >= self.bytes.len() {
            return &[];
        }
        &self.bytes[local_offset..]
    }

    /// Drop oldest bytes until `self.bytes.len() <= self.max_bytes`.
    fn trim(&mut self) {
        if self.bytes.len() <= self.max_bytes {
            return;
        }
        let excess = self.bytes.len() - self.max_bytes;
        self.bytes.drain(..excess);
        self.base_offset += excess;
    }
}

/// Run the log relay on `vsock_port` until the VM shuts down.
pub fn serve(vsock_port: u32) -> io::Result<()> {
    serve_inner(vsock_port, None)
}

/// Run the log relay and signal readiness after successful bind + listen.
pub fn serve_with_ready(
    vsock_port: u32,
    ready_tx: std::sync::mpsc::Sender<io::Result<()>>,
) -> io::Result<()> {
    serve_inner(vsock_port, Some(ready_tx))
}

fn serve_inner(
    vsock_port: u32,
    ready_tx: Option<std::sync::mpsc::Sender<io::Result<()>>>,
) -> io::Result<()> {
    let listen_fd = match create_listener(vsock_port) {
        Ok(fd) => {
            if let Some(tx) = ready_tx {
                let _ = tx.send(Ok(()));
            }
            fd
        }
        Err(err) => {
            if let Some(tx) = ready_tx {
                let _ = tx.send(Err(io::Error::new(err.kind(), err.to_string())));
            }
            return Err(err);
        }
    };

    let buffers: LogBuffers = Arc::new(Mutex::new(HashMap::new()));

    loop {
        let conn_fd =
            unsafe { libc::accept(listen_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if conn_fd < 0 {
            eprintln!("log_relay: accept failed: {}", io::Error::last_os_error());
            continue;
        }

        let buffers = Arc::clone(&buffers);
        std::thread::spawn(move || handle_connection(conn_fd, buffers));
    }
}

fn create_listener(vsock_port: u32) -> io::Result<libc::c_int> {
    let listen_fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if listen_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let addr = libc::sockaddr_vm {
        svm_family: libc::AF_VSOCK as u16,
        svm_reserved1: 0,
        svm_port: vsock_port,
        svm_cid: libc::VMADDR_CID_ANY,
        svm_zero: [0u8; 4],
    };

    let addr_ptr = &addr as *const libc::sockaddr_vm as *const libc::sockaddr;
    let addr_len = std::mem::size_of::<libc::sockaddr_vm>() as u32;

    if unsafe { libc::bind(listen_fd, addr_ptr, addr_len) } < 0 {
        let err = io::Error::last_os_error();
        unsafe { libc::close(listen_fd) };
        return Err(err);
    }

    if unsafe { libc::listen(listen_fd, 16) } < 0 {
        let err = io::Error::last_os_error();
        unsafe { libc::close(listen_fd) };
        return Err(err);
    }

    Ok(listen_fd)
}

fn handle_connection(conn_fd: libc::c_int, buffers: LogBuffers) {
    let result =
        read_command_line(conn_fd).and_then(|command| handle_command(conn_fd, &buffers, &command));
    if result.is_err() {
        let _ = write_all_fd(conn_fd, b"ERR\n");
    }
    unsafe { libc::close(conn_fd) };
}

fn handle_command(conn_fd: libc::c_int, buffers: &LogBuffers, command: &str) -> io::Result<()> {
    if let Some(container_id) = command.strip_prefix("CREATE:") {
        validate_container_id(container_id)?;
        create_fifos(conn_fd, buffers, container_id)
    } else if let Some(rest) = command.strip_prefix("READ:") {
        let mut parts = rest.splitn(3, ':');
        let stream = parts.next().unwrap_or_default();
        let container_id = parts.next().unwrap_or_default();
        let offset = match parts.next() {
            Some(raw) => raw.parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid read offset")
            })?,
            None => 0,
        };
        validate_container_id(container_id)?;
        read_stream(conn_fd, buffers, stream, container_id, offset)
    } else if let Some(container_id) = command.strip_prefix("CLOSE:") {
        validate_container_id(container_id)?;
        buffers
            .lock()
            .expect("log buffers mutex poisoned")
            .remove(container_id);
        remove_fifos(container_id);
        write_all_fd(conn_fd, b"OK\n")
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown command",
        ))
    }
}

fn validate_container_id(container_id: &str) -> io::Result<()> {
    if container_id.is_empty()
        || !container_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid container id",
        ));
    }
    Ok(())
}

fn create_fifos(conn_fd: libc::c_int, buffers: &LogBuffers, container_id: &str) -> io::Result<()> {
    std::fs::create_dir_all("/rootfs/tmp/speck-logs/")?;
    let stdout_path = format!("/rootfs/tmp/speck-logs/{container_id}.stdout");
    let stderr_path = format!("/rootfs/tmp/speck-logs/{container_id}.stderr");
    mkfifo_if_needed(&stdout_path)?;
    mkfifo_if_needed(&stderr_path)?;

    let container_buffers = ContainerLogBuffers {
        stdout: Arc::new(Mutex::new(RetainedLogBuffer::new(MAX_LOG_STREAM_BYTES))),
        stderr: Arc::new(Mutex::new(RetainedLogBuffer::new(MAX_LOG_STREAM_BYTES))),
    };
    let stdout_buffer = Arc::clone(&container_buffers.stdout);
    let stderr_buffer = Arc::clone(&container_buffers.stderr);

    buffers
        .lock()
        .expect("log buffers mutex poisoned")
        .insert(container_id.to_owned(), container_buffers);

    std::thread::spawn(move || reader_thread(stdout_path, stdout_buffer));
    std::thread::spawn(move || reader_thread(stderr_path, stderr_buffer));

    write_all_fd(conn_fd, b"OK\n")
}

fn mkfifo_if_needed(path: &str) -> io::Result<()> {
    let path_cstr = CString::new(path)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "fifo path contains NUL"))?;
    if unsafe { libc::mkfifo(path_cstr.as_ptr(), 0o600) } < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EEXIST) {
            return Err(err);
        }
    }
    Ok(())
}

fn reader_thread(fifo_path: String, buffer: Arc<Mutex<RetainedLogBuffer>>) {
    let path_cstr = match CString::new(fifo_path) {
        Ok(path) => path,
        Err(_) => return,
    };
    let fd = unsafe { libc::open(path_cstr.as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        eprintln!(
            "log_relay: FIFO open failed: {}",
            io::Error::last_os_error()
        );
        return;
    }

    let mut read_buf = [0u8; 4096];
    loop {
        let n = unsafe {
            libc::read(
                fd,
                read_buf.as_mut_ptr() as *mut libc::c_void,
                read_buf.len(),
            )
        };
        if n <= 0 {
            break;
        }
        buffer
            .lock()
            .expect("log buffer mutex poisoned")
            .append(&read_buf[..n as usize]);
    }

    unsafe { libc::close(fd) };
}

/// Remove a container's FIFO files so a retried CREATE mints fresh inodes.
///
/// Without unlinking, a second CREATE for the same container ID hits `mkfifo`
/// EEXIST, spawns a second reader pair on the same FIFO inode, and the old
/// blocked reader steals bytes from the new one.
fn remove_fifos(container_id: &str) {
    for suffix in ["stdout", "stderr"] {
        let path = format!("/rootfs/tmp/speck-logs/{container_id}.{suffix}");
        if let Err(err) = std::fs::remove_file(&path)
            && err.kind() != io::ErrorKind::NotFound
        {
            eprintln!("log_relay: FIFO unlink failed for {path}: {err}");
        }
    }
}

fn read_stream(
    conn_fd: libc::c_int,
    buffers: &LogBuffers,
    stream: &str,
    container_id: &str,
    offset: usize,
) -> io::Result<()> {
    let buffer_arc = {
        let guard = buffers.lock().expect("log buffers mutex poisoned");
        let Some(container_buffers) = guard.get(container_id) else {
            return write_length_prefixed(conn_fd, &[]);
        };
        match stream {
            "stdout" => Arc::clone(&container_buffers.stdout),
            "stderr" => Arc::clone(&container_buffers.stderr),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid stream",
                ));
            }
        }
    };

    let guard = buffer_arc.lock().expect("log buffer mutex poisoned");
    let slice = guard.read_from(offset);
    // Clone the slice so we drop the lock before writing to the socket.
    let bytes = slice.to_vec();
    drop(guard);
    write_length_prefixed(conn_fd, &bytes)
}

fn write_length_prefixed(conn_fd: libc::c_int, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "log buffer too large"))?;
    write_all_fd(conn_fd, &len.to_be_bytes())?;
    write_all_fd(conn_fd, bytes)
}

fn read_command_line(conn_fd: libc::c_int) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    while bytes.len() < 512 {
        let n = unsafe { libc::read(conn_fd, byte.as_mut_ptr() as *mut libc::c_void, 1) };
        if n <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "missing command",
            ));
        }
        if byte[0] == b'\n' {
            return String::from_utf8(bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "command is not UTF-8"));
        }
        bytes.push(byte[0]);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "command too long",
    ))
}

fn write_all_fd(fd: libc::c_int, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "write returned zero",
            ));
        }
        buf = &buf[n as usize..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retained_buffer_appends_and_trim() {
        let mut buf = RetainedLogBuffer::new(16);
        assert_eq!(buf.base_offset, 0);
        assert!(buf.bytes.is_empty());

        buf.append(b"hello ");
        assert_eq!(buf.read_from(0), b"hello ");
        assert_eq!(buf.base_offset, 0);

        buf.append(b"world");
        assert_eq!(buf.read_from(0), b"hello world");
    }

    #[test]
    fn test_retained_buffer_trim_drops_oldest() {
        let mut buf = RetainedLogBuffer::new(10);
        buf.append(b"0123456789ABCDEF");
        // 16 bytes written, max is 10.
        assert_eq!(buf.bytes.len(), 10, "should be trimmed to max_bytes");
        assert_eq!(buf.base_offset, 6, "excess 6 bytes trimmed");
        assert_eq!(buf.read_from(6), b"6789ABCDEF", "read from new base");
    }

    #[test]
    fn test_retained_buffer_read_before_base_returns_all() {
        let mut buf = RetainedLogBuffer::new(5);
        buf.append(b"abcdefghij");
        assert_eq!(buf.base_offset, 5, "5 bytes trimmed");
        // Reading from offset 0 returns whatever we have (best effort).
        assert_eq!(buf.read_from(0), b"fghij");
    }

    #[test]
    fn test_retained_buffer_read_past_end_returns_empty() {
        let mut buf = RetainedLogBuffer::new(100);
        buf.append(b"hello");
        assert!(buf.read_from(100).is_empty());
    }

    #[test]
    fn test_retained_buffer_offset_semantics() {
        let mut buf = RetainedLogBuffer::new(20);
        // Append chunks that cross the boundary.
        buf.append(b"AAAA" );
        buf.append(b"BBBB" );
        buf.append(b"CCCC" );
        buf.append(b"DDDD" );
        buf.append(b"EEEE" );
        buf.append(b"FFFF" );
        // 24 bytes total, max is 20, so 4 trimmed.
        assert_eq!(buf.base_offset, 4);
        assert_eq!(buf.read_from(4), b"BBBBCCCCDDDDEEEEFFFF");
        assert_eq!(buf.read_from(8), b"CCCCDDDDEEEEFFFF");
        assert_eq!(buf.read_from(24), b"");
    }
}
