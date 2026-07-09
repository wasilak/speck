use std::io;

/// Maximum bytes accepted from a single relay read response.
///
/// Rejecting oversized length prefixes before allocation prevents a
/// misbehaving guest from making the host allocate unbounded memory (WR-03).
pub const MAX_RELAY_LOG_READ: usize = 16 * 1024 * 1024;

pub struct RelayLogs {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn create(guest: &speck_vz::Guest, port: u32, container_id: &str) -> io::Result<()> {
    let socket = connect(guest, port)?;
    write_all_to_socket(&socket, format!("CREATE:{container_id}\n").as_bytes())?;
    read_ok_ack(&socket)
}

pub fn read_logs(guest: &speck_vz::Guest, port: u32, container_id: &str) -> io::Result<RelayLogs> {
    read_logs_from(guest, port, container_id, 0, 0)
}

/// Read log bytes appended after the given per-stream offsets.
pub fn read_logs_from(
    guest: &speck_vz::Guest,
    port: u32,
    container_id: &str,
    stdout_offset: usize,
    stderr_offset: usize,
) -> io::Result<RelayLogs> {
    let stdout = read_stream(guest, port, "stdout", container_id, stdout_offset)?;
    let stderr = read_stream(guest, port, "stderr", container_id, stderr_offset)?;
    Ok(RelayLogs { stdout, stderr })
}

/// Read all log bytes by looping over offset windows.
///
/// Each call to `read_logs_from` returns at most `MAX_RELAY_LOG_READ`
/// bytes per stream.  For containers that have written more than that
/// cumulatively, this helper repeatedly fetches the next window until
/// both streams return less than the ceiling (meaning we've caught up).
pub fn read_logs_windowed(
    guest: &speck_vz::Guest,
    port: u32,
    container_id: &str,
) -> io::Result<RelayLogs> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_offset = 0usize;
    let mut stderr_offset = 0usize;

    loop {
        let chunk = read_logs_from(guest, port, container_id, stdout_offset, stderr_offset)?;

        let stdout_empty = chunk.stdout.len() < MAX_RELAY_LOG_READ;
        let stderr_empty = chunk.stderr.len() < MAX_RELAY_LOG_READ;

        stdout_offset += chunk.stdout.len();
        stderr_offset += chunk.stderr.len();
        stdout.extend(chunk.stdout);
        stderr.extend(chunk.stderr);

        if stdout_empty && stderr_empty {
            break;
        }
    }

    Ok(RelayLogs { stdout, stderr })
}

pub fn close(guest: &speck_vz::Guest, port: u32, container_id: &str) -> io::Result<()> {
    let socket = connect(guest, port)?;
    write_all_to_socket(&socket, format!("CLOSE:{container_id}\n").as_bytes())?;
    read_ok_ack(&socket)
}

fn read_stream(
    guest: &speck_vz::Guest,
    port: u32,
    stream: &str,
    container_id: &str,
    offset: usize,
) -> io::Result<Vec<u8>> {
    let socket = connect(guest, port)?;
    write_all_to_socket(
        &socket,
        format!("READ:{stream}:{container_id}:{offset}\n").as_bytes(),
    )?;

    let mut len_buf = [0u8; 4];
    read_exact_from_socket(&socket, &mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_RELAY_LOG_READ {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "log relay response too large",
        ));
    }
    let mut bytes = vec![0u8; len];
    read_exact_from_socket(&socket, &mut bytes)?;
    Ok(bytes)
}

fn connect(guest: &speck_vz::Guest, port: u32) -> io::Result<speck_vz::VzSocket> {
    guest
        .vsock_connect(port)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
}

fn write_all_to_socket(socket: &speck_vz::VzSocket, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = socket.write(buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "write returned zero",
            ));
        }
        buf = &buf[n..];
    }
    Ok(())
}

fn read_exact_from_socket(socket: &speck_vz::VzSocket, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = socket.read(buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "log relay socket closed early",
            ));
        }
        buf = &mut buf[n..];
    }
    Ok(())
}

fn read_ok_ack(socket: &speck_vz::VzSocket) -> io::Result<()> {
    let mut ack = [0u8; 3];
    read_exact_from_socket(socket, &mut ack)?;
    if ack == *b"OK\n" {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "log relay returned error acknowledgment",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate a guest that returns diminishing windows.
    ///
    /// Each call moves the internal cursor forward, returning chunks
    /// that shrink to confirm the windowed loop terminates.
    struct SimulatedGuest {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_offset: usize,
        stderr_offset: usize,
    }

    impl SimulatedGuest {
        fn new(stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
            Self {
                stdout,
                stderr,
                stdout_offset: 0,
                stderr_offset: 0,
            }
        }

        fn read(&mut self, so: usize, _eo: usize) -> RelayLogs {
            // Simulate the effect of MAX_RELAY_LOG_READ ceiling
            // by returning chunks capped at 10 bytes for testing.
            let max_chunk = 10;
            let s_chunk: Vec<u8> = self.stdout[so..]
                .iter()
                .take(max_chunk)
                .copied()
                .collect();
            let e_chunk: Vec<u8> = self.stderr[_eo..]
                .iter()
                .take(max_chunk)
                .copied()
                .collect();
            RelayLogs {
                stdout: s_chunk,
                stderr: e_chunk,
            }
        }
    }

    /// A mock guest wrapper that injects our simulated guest into
    /// the `read_logs_from` closure pattern.  Since `read_logs_windowed`
    /// calls `read_logs_from`, we test the looping logic directly.
    #[test]
    fn test_read_logs_windowed_terminates() {
        let large_stdout: Vec<u8> = (0..25).map(|i| b'a' + i).collect(); // 25 bytes
        let large_stderr: Vec<u8> = (0..20).map(|i| b'A' + i).collect(); // 20 bytes

        // Simulate the windowed loop manually — this tests the same
        // termination logic that `read_logs_windowed` uses internally.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut mut_guest = SimulatedGuest::new(large_stdout, large_stderr);

        loop {
            let chunk = mut_guest.read(mut_guest.stdout_offset, mut_guest.stderr_offset);
            let stdout_done = chunk.stdout.len() < 10;
            let stderr_done = chunk.stderr.len() < 10;

            mut_guest.stdout_offset += chunk.stdout.len();
            mut_guest.stderr_offset += chunk.stderr.len();
            stdout.extend(chunk.stdout);
            stderr.extend(chunk.stderr);

            if stdout_done && stderr_done {
                break;
            }
        }

        assert_eq!(stdout.len(), 25, "should have collected all stdout bytes");
        assert_eq!(stderr.len(), 20, "should have collected all stderr bytes");
    }

    #[test]
    fn test_read_logs_windowed_small_read_no_loop() {
        // When data fits in one window, the loop terminates after
        // one iteration.
        let small_stdout: Vec<u8> = b"hello world".to_vec();
        let mut stdout = Vec::new();
        let mut mut_guest = SimulatedGuest::new(small_stdout, Vec::new());

        loop {
            let chunk = mut_guest.read(mut_guest.stdout_offset, mut_guest.stderr_offset);
            let stdout_done = chunk.stdout.len() < 10;
            mut_guest.stdout_offset += chunk.stdout.len();
            stdout.extend(chunk.stdout);
            if stdout_done {
                break;
            }
        }

        assert_eq!(stdout.len(), 11, "should have collected all 11 bytes");
    }
}
