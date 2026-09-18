use std::io::{Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::MAX_FRAME_SIZE;

const LENGTH_PREFIX_SIZE: usize = size_of::<u32>();

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame payload is {size} bytes, maximum is {max}")]
    TooLarge { size: usize, max: usize },
    #[error("frame I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    validate_size(payload.len())?;
    let length = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge {
        size: payload.len(),
        max: MAX_FRAME_SIZE,
    })?;
    let mut frame = Vec::with_capacity(LENGTH_PREFIX_SIZE + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameError> {
    encode_frame(&serde_json::to_vec(value)?)
}

pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), FrameError> {
    validate_size(payload.len())?;
    let length = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge {
        size: payload.len(),
        max: MAX_FRAME_SIZE,
    })?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(payload)?;
    Ok(())
}

pub fn write_json<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), FrameError> {
    let payload = serde_json::to_vec(value)?;
    write_frame(writer, &payload)
}

pub fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, FrameError> {
    let mut prefix = [0_u8; LENGTH_PREFIX_SIZE];
    reader.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    validate_size(length)?;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

pub fn read_json<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, FrameError> {
    let payload = read_frame(reader)?;
    Ok(serde_json::from_slice(&payload)?)
}

fn validate_size(size: usize) -> Result<(), FrameError> {
    if size > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge {
            size,
            max: MAX_FRAME_SIZE,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};

    use serde_json::json;

    use super::*;
    use crate::{PROTOCOL_VERSION, RequestEnvelope};

    #[test]
    fn frame_prefix_is_u32_big_endian() {
        let frame = encode_frame(b"hello").unwrap();
        assert_eq!(&frame[..4], &[0, 0, 0, 5]);
        assert_eq!(&frame[4..], b"hello");
    }

    #[test]
    fn truncated_prefix_and_payload_are_io_errors() {
        for bytes in [vec![0, 0, 0], vec![0, 0, 0, 5, b'a', b'b']] {
            let error = read_frame(&mut Cursor::new(bytes)).unwrap_err();
            assert!(matches!(
                error,
                FrameError::Io(ref io) if io.kind() == ErrorKind::UnexpectedEof
            ));
        }
    }

    #[test]
    fn oversized_length_is_rejected_before_payload_read() {
        let length = u32::try_from(MAX_FRAME_SIZE + 1).unwrap();
        let error = read_frame(&mut Cursor::new(length.to_be_bytes())).unwrap_err();
        assert!(matches!(
            error,
            FrameError::TooLarge { size, max }
                if size == MAX_FRAME_SIZE + 1 && max == MAX_FRAME_SIZE
        ));

        let payload = vec![0_u8; MAX_FRAME_SIZE + 1];
        assert!(matches!(
            encode_frame(&payload),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn consecutive_frames_are_read_without_consuming_the_next_one() {
        let mut bytes = encode_frame(b"one").unwrap();
        bytes.extend(encode_frame(b"two").unwrap());
        let mut cursor = Cursor::new(bytes);
        assert_eq!(read_frame(&mut cursor).unwrap(), b"one");
        assert_eq!(read_frame(&mut cursor).unwrap(), b"two");
    }

    #[test]
    fn json_frame_roundtrip_preserves_request() {
        let request = RequestEnvelope {
            request_id: "req-json".into(),
            method: "device.info".into(),
            params: json!({}),
            timeout_ms: 1_500,
            protocol_version: PROTOCOL_VERSION,
        };
        let frame = encode_json(&request).unwrap();
        let decoded: RequestEnvelope = read_json(&mut Cursor::new(frame)).unwrap();
        assert_eq!(decoded, request);

        let mut written = Vec::new();
        write_json(&mut written, &request).unwrap();
        assert_eq!(written, encode_json(&request).unwrap());
    }
}
