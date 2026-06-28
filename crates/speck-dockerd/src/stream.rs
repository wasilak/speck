use tokio::io::{AsyncRead, AsyncReadExt};

pub fn encode_frame(stream_type: u8, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(8 + data.len());
    frame.push(stream_type);
    frame.extend_from_slice(&[0, 0, 0]);
    frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
    frame.extend_from_slice(data);
    frame
}

pub async fn decode_frame<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<(u8, Vec<u8>)> {
    let mut header = [0_u8; 8];
    reader.read_exact(&mut header).await?;

    let stream_type = header[0];
    let payload_len = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let mut payload = vec![0_u8; payload_len];
    reader.read_exact(&mut payload).await?;

    Ok((stream_type, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_stdout_hello() {
        let frame = encode_frame(1, b"hello");

        assert_eq!(frame[0], 1);
        assert_eq!(&frame[1..4], &[0, 0, 0]);
        assert_eq!(&frame[4..8], &5_u32.to_be_bytes());
        assert_eq!(&frame[8..], b"hello");
    }

    #[tokio::test]
    async fn decode_round_trip_stdin() {
        let frame = encode_frame(0, b"cmd");
        let mut reader = std::io::Cursor::new(frame);

        let decoded = decode_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, (0, b"cmd".to_vec()));
    }

    #[tokio::test]
    async fn decode_round_trip_stderr() {
        let frame = encode_frame(2, b"err message");
        let mut reader = std::io::Cursor::new(frame);

        let decoded = decode_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, (2, b"err message".to_vec()));
    }
}
