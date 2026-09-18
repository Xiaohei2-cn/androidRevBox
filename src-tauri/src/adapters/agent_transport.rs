use std::io::ErrorKind;

use agent_protocol::{MAX_FRAME_SIZE, RequestEnvelope, ResponseEnvelope, encode_json};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentTransportError {
    #[error("agent transport closed")]
    Closed,
    #[error("agent transport I/O failed: {0}")]
    Io(String),
    #[error("agent frame is {size} bytes, maximum is {max}")]
    FrameTooLarge { size: usize, max: usize },
    #[error("agent frame JSON is invalid: {0}")]
    InvalidFrame(String),
}

#[async_trait]
pub trait AgentTransport: Send + Sync {
    async fn send(&self, request: &RequestEnvelope) -> Result<(), AgentTransportError>;

    async fn receive(&self) -> Result<ResponseEnvelope, AgentTransportError>;
}

pub struct FramedAgentTransport<S> {
    reader: Mutex<ReadHalf<S>>,
    writer: Mutex<WriteHalf<S>>,
}

impl<S> FramedAgentTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(stream: S) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
        }
    }
}

#[async_trait]
impl<S> AgentTransport for FramedAgentTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn send(&self, request: &RequestEnvelope) -> Result<(), AgentTransportError> {
        let frame = encode_json(request).map_err(|error| match error {
            agent_protocol::FrameError::TooLarge { size, max } => {
                AgentTransportError::FrameTooLarge { size, max }
            }
            other => AgentTransportError::InvalidFrame(other.to_string()),
        })?;
        self.writer
            .lock()
            .await
            .write_all(&frame)
            .await
            .map_err(map_io_error)
    }

    async fn receive(&self) -> Result<ResponseEnvelope, AgentTransportError> {
        read_message(&mut *self.reader.lock().await).await
    }
}

async fn read_message<R, T>(reader: &mut R) -> Result<T, AgentTransportError>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).await.map_err(map_io_error)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_FRAME_SIZE {
        return Err(AgentTransportError::FrameTooLarge {
            size: length,
            max: MAX_FRAME_SIZE,
        });
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(map_io_error)?;
    serde_json::from_slice(&payload)
        .map_err(|error| AgentTransportError::InvalidFrame(error.to_string()))
}

fn map_io_error(error: std::io::Error) -> AgentTransportError {
    if matches!(
        error.kind(),
        ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
    ) {
        AgentTransportError::Closed
    } else {
        AgentTransportError::Io(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use agent_protocol::{PROTOCOL_VERSION, ResponseEnvelope, encode_json};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn request() -> RequestEnvelope {
        RequestEnvelope {
            request_id: "req-1".into(),
            method: "system.health".into(),
            params: json!({}),
            timeout_ms: 1_000,
            protocol_version: PROTOCOL_VERSION,
        }
    }

    #[tokio::test]
    async fn framed_transport_writes_requests_and_reads_responses() {
        let (desktop, mut agent) = tokio::io::duplex(8 * 1024);
        let transport = FramedAgentTransport::new(desktop);
        let peer = tokio::spawn(async move {
            let mut prefix = [0_u8; 4];
            agent.read_exact(&mut prefix).await.unwrap();
            let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
            agent.read_exact(&mut payload).await.unwrap();
            let request: RequestEnvelope = serde_json::from_slice(&payload).unwrap();
            let response = ResponseEnvelope::success(request.request_id, json!({ "ok": true }));
            agent
                .write_all(&encode_json(&response).unwrap())
                .await
                .unwrap();
        });

        transport.send(&request()).await.unwrap();
        let response = transport.receive().await.unwrap();
        assert_eq!(response.request_id, "req-1");
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_prefix_is_rejected_before_payload_allocation() {
        let (desktop, mut agent) = tokio::io::duplex(64);
        let transport = FramedAgentTransport::new(desktop);
        let size = u32::try_from(MAX_FRAME_SIZE + 1).unwrap();
        agent.write_all(&size.to_be_bytes()).await.unwrap();
        let error = transport.receive().await.unwrap_err();
        assert_eq!(
            error,
            AgentTransportError::FrameTooLarge {
                size: MAX_FRAME_SIZE + 1,
                max: MAX_FRAME_SIZE,
            }
        );
    }
}
