use std::io;

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
    let stdout = read_stream(guest, port, "stdout", container_id)?;
    let stderr = read_stream(guest, port, "stderr", container_id)?;
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
) -> io::Result<Vec<u8>> {
    let socket = connect(guest, port)?;
    write_all_to_socket(
        &socket,
        format!("READ:{stream}:{container_id}\n").as_bytes(),
    )?;

    let mut len_buf = [0u8; 4];
    read_exact_from_socket(&socket, &mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
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
