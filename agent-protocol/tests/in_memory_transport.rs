use std::collections::HashMap;
use std::future::pending;
use std::io::{Error, ErrorKind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_protocol::{
    AgentError, ErrorCode, MAX_FRAME_SIZE, PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope,
    ResponsePayload, encode_json,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

type PendingResult = Result<ResponseEnvelope, AgentError>;
type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<PendingResult>>>>;

#[derive(Clone)]
struct MemoryClient {
    writer: Arc<AsyncMutex<tokio::io::WriteHalf<DuplexStream>>>,
    pending: PendingMap,
    next_id: Arc<AtomicU64>,
}

impl MemoryClient {
    fn new(stream: DuplexStream) -> Self {
        let (mut reader, writer) = tokio::io::split(stream);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            loop {
                match read_message::<_, ResponseEnvelope>(&mut reader).await {
                    Ok(response) => {
                        let sender = reader_pending
                            .lock()
                            .expect("pending lock")
                            .remove(&response.request_id);
                        if let Some(sender) = sender {
                            let _ = sender.send(Ok(response));
                        }
                    }
                    Err(error) => {
                        let waiters =
                            std::mem::take(&mut *reader_pending.lock().expect("pending lock"));
                        for (_, sender) in waiters {
                            let _ = sender.send(Err(AgentError::new(
                                ErrorCode::TransportLost,
                                format!("in-memory transport lost: {error}"),
                            )));
                        }
                        break;
                    }
                }
            }
        });
        Self {
            writer: Arc::new(AsyncMutex::new(writer)),
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    async fn request(
        &self,
        method: &str,
        timeout: Duration,
        cancel: Option<oneshot::Receiver<()>>,
    ) -> PendingResult {
        let request_id = format!("req-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let request = RequestEnvelope {
            request_id: request_id.clone(),
            method: method.into(),
            params: json!({}),
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            protocol_version: PROTOCOL_VERSION,
        };
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending lock")
            .insert(request_id.clone(), sender);

        if let Err(error) = write_message(&mut *self.writer.lock().await, &request).await {
            self.remove_pending(&request_id);
            return Err(AgentError::new(
                ErrorCode::TransportLost,
                format!("request write failed: {error}"),
            ));
        }

        let cancel_wait = async move {
            match cancel {
                Some(receiver) => {
                    let _ = receiver.await;
                }
                None => pending::<()>().await,
            }
        };
        tokio::select! {
            response = receiver => response.unwrap_or_else(|_| {
                Err(AgentError::new(ErrorCode::TransportLost, "response waiter dropped"))
            }),
            _ = tokio::time::sleep(timeout) => {
                self.remove_pending(&request_id);
                Err(AgentError::new(ErrorCode::DeadlineExceeded, "request deadline exceeded"))
            },
            _ = cancel_wait => {
                self.remove_pending(&request_id);
                Err(AgentError::new(ErrorCode::Cancelled, "request cancelled"))
            }
        }
    }

    fn remove_pending(&self, request_id: &str) {
        self.pending
            .lock()
            .expect("pending lock")
            .remove(request_id);
    }

    fn pending_count(&self) -> usize {
        self.pending.lock().expect("pending lock").len()
    }
}

async fn write_message<W, T>(writer: &mut W, value: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let frame = encode_json(value).map_err(|error| Error::new(ErrorKind::InvalidData, error))?;
    writer.write_all(&frame).await
}

async fn read_message<R, T>(reader: &mut R) -> std::io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_FRAME_SIZE {
        return Err(Error::new(ErrorKind::InvalidData, "frame exceeds limit"));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).map_err(|error| Error::new(ErrorKind::InvalidData, error))
}

fn result_method(response: ResponseEnvelope) -> String {
    let ResponsePayload::Success { result } = response.payload else {
        panic!("expected success response");
    };
    result["method"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiplexes_out_of_order_responses_by_request_id() {
    let (desktop, mut agent) = tokio::io::duplex(16 * 1024);
    let client = MemoryClient::new(desktop);
    let fake_agent = tokio::spawn(async move {
        let first: RequestEnvelope = read_message(&mut agent).await.unwrap();
        let second: RequestEnvelope = read_message(&mut agent).await.unwrap();
        write_message(
            &mut agent,
            &ResponseEnvelope::success(second.request_id, json!({ "method": second.method })),
        )
        .await
        .unwrap();
        write_message(
            &mut agent,
            &ResponseEnvelope::success(first.request_id, json!({ "method": first.method })),
        )
        .await
        .unwrap();
    });

    let first_client = client.clone();
    let first = tokio::spawn(async move {
        first_client
            .request("method.first", Duration::from_secs(1), None)
            .await
    });
    let second_client = client.clone();
    let second = tokio::spawn(async move {
        second_client
            .request("method.second", Duration::from_secs(1), None)
            .await
    });

    let first_method = result_method(first.await.unwrap().unwrap());
    let second_method = result_method(second.await.unwrap().unwrap());
    assert_eq!(first_method, "method.first");
    assert_eq!(second_method, "method.second");
    assert_eq!(client.pending_count(), 0);
    fake_agent.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadline_removes_pending_request() {
    let (desktop, mut agent) = tokio::io::duplex(4 * 1024);
    let client = MemoryClient::new(desktop);
    let (release_sender, release_receiver) = oneshot::channel();
    let fake_agent = tokio::spawn(async move {
        let _: RequestEnvelope = read_message(&mut agent).await.unwrap();
        let _ = release_receiver.await;
    });

    let error = client
        .request("method.slow", Duration::from_millis(25), None)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::DeadlineExceeded);
    assert_eq!(client.pending_count(), 0);
    let _ = release_sender.send(());
    fake_agent.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_wakes_every_pending_request() {
    let (desktop, mut agent) = tokio::io::duplex(4 * 1024);
    let client = MemoryClient::new(desktop);
    let fake_agent = tokio::spawn(async move {
        let _: RequestEnvelope = read_message(&mut agent).await.unwrap();
        let _: RequestEnvelope = read_message(&mut agent).await.unwrap();
        drop(agent);
    });

    let one_client = client.clone();
    let one = tokio::spawn(async move {
        one_client
            .request("method.one", Duration::from_secs(1), None)
            .await
    });
    let two_client = client.clone();
    let two = tokio::spawn(async move {
        two_client
            .request("method.two", Duration::from_secs(1), None)
            .await
    });

    for result in [one.await.unwrap(), two.await.unwrap()] {
        assert_eq!(result.unwrap_err().code, ErrorCode::TransportLost);
    }
    assert_eq!(client.pending_count(), 0);
    fake_agent.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_removes_waiter_without_closing_transport() {
    let (desktop, mut agent) = tokio::io::duplex(4 * 1024);
    let client = MemoryClient::new(desktop);
    let (seen_sender, seen_receiver) = oneshot::channel();
    let (release_sender, release_receiver) = oneshot::channel();
    let fake_agent = tokio::spawn(async move {
        let _: RequestEnvelope = read_message(&mut agent).await.unwrap();
        let _ = seen_sender.send(());
        let _ = release_receiver.await;
    });
    let (cancel_sender, cancel_receiver) = oneshot::channel();
    let request_client = client.clone();
    let request = tokio::spawn(async move {
        request_client
            .request(
                "method.cancel",
                Duration::from_secs(1),
                Some(cancel_receiver),
            )
            .await
    });

    seen_receiver.await.unwrap();
    cancel_sender.send(()).unwrap();
    let error = request.await.unwrap().unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(client.pending_count(), 0);
    let _ = release_sender.send(());
    fake_agent.await.unwrap();
}
