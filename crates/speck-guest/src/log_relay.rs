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

type LogBuffers = Arc<Mutex<HashMap<String, ContainerLogBuffers>>>;

#[derive(Clone)]
struct ContainerLogBuffers {
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
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
        let mut parts = rest.splitn(2, ':');
        let stream = parts.next().unwrap_or_default();
        let container_id = parts.next().unwrap_or_default();
        validate_container_id(container_id)?;
        read_stream(conn_fd, buffers, stream, container_id)
    } else if let Some(container_id) = command.strip_prefix("CLOSE:") {
        validate_container_id(container_id)?;
        buffers
            .lock()
            .expect("log buffers mutex poisoned")
            .remove(container_id);
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
        stdout: Arc::new(Mutex::new(Vec::new())),
        stderr: Arc::new(Mutex::new(Vec::new())),
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

fn reader_thread(fifo_path: String, buffer: Arc<Mutex<Vec<u8>>>) {
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
            .extend_from_slice(&read_buf[..n as usize]);
    }

    unsafe { libc::close(fd) };
}

fn read_stream(
    conn_fd: libc::c_int,
    buffers: &LogBuffers,
    stream: &str,
    container_id: &str,
) -> io::Result<()> {
    let stream_buffer = {
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

    let bytes = stream_buffer
        .lock()
        .expect("log buffer mutex poisoned")
        .clone();
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
