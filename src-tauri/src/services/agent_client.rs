use std::collections::HashMap;
use std::future::pending;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_protocol::method::{CAPABILITY_LIST, SYSTEM_HEALTH, SYSTEM_HELLO};
use agent_protocol::{
    AgentError, CapabilityListResult, EmptyParams, HealthResult, HelloParams, HelloResult,
    PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope, ResponsePayload,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::adapters::agent_transport::{AgentTransport, AgentTransportError};

type PendingResult = Result<ResponseEnvelope, AgentClientError>;
type SharedState = Arc<Mutex<ClientState>>;

#[derive(Default)]
struct ClientState {
    pending: HashMap<String, oneshot::Sender<PendingResult>>,
    terminal_error: Option<AgentClientError>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AgentClientError {
    #[error("agent transport lost: {0}")]
    TransportLost(String),
    #[error("agent request deadline exceeded")]
    DeadlineExceeded,
    #[error("agent request cancelled")]
    Cancelled,
    #[error("agent protocol error: {0}")]
    Protocol(String),
    #[error("agent returned {code}: {message}", code = .0.code, message = .0.message)]
    Remote(AgentError),
}

impl From<AgentTransportError> for AgentClientError {
    fn from(error: AgentTransportError) -> Self {
        Self::TransportLost(error.to_string())
    }
}

#[derive(Clone)]
pub struct AgentClient {
    inner: Arc<AgentClientInner>,
}

struct AgentClientInner {
    transport: Arc<dyn AgentTransport>,
    state: SharedState,
    next_request_id: AtomicU64,
    reader_task: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for AgentClientInner {
    fn drop(&mut self) {
        if let Some(task) = self
            .reader_task
            .lock()
            .expect("agent reader task lock poisoned")
            .take()
        {
            task.abort();
        }
    }
}

impl AgentClient {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        let state = Arc::new(Mutex::new(ClientState::default()));
        let reader_task = tokio::spawn(reader_loop(transport.clone(), state.clone()));
        Self {
            inner: Arc::new(AgentClientInner {
                transport,
                state,
                next_request_id: AtomicU64::new(1),
                reader_task: Mutex::new(Some(reader_task)),
            }),
        }
    }

    pub async fn hello(
        &self,
        auth_token: impl Into<String>,
        desktop_version: impl Into<String>,
        timeout: Duration,
    ) -> Result<HelloResult, AgentClientError> {
        self.request(
            SYSTEM_HELLO,
            &HelloParams::v1(auth_token, desktop_version),
            timeout,
        )
        .await
    }

    pub async fn health(&self, timeout: Duration) -> Result<HealthResult, AgentClientError> {
        self.request(SYSTEM_HEALTH, &EmptyParams {}, timeout).await
    }

    pub async fn capabilities(
        &self,
        timeout: Duration,
    ) -> Result<CapabilityListResult, AgentClientError> {
        self.request(CAPABILITY_LIST, &EmptyParams {}, timeout)
            .await
    }

    pub fn disconnect(&self, reason: impl Into<String>) {
        fail_all(
            &self.inner.state,
            AgentClientError::TransportLost(reason.into()),
        );
        if let Some(task) = self
            .inner
            .reader_task
            .lock()
            .expect("agent reader task lock poisoned")
            .take()
        {
            task.abort();
        }
    }

    pub async fn request<P, R>(
        &self,
        method: &str,
        params: &P,
        timeout: Duration,
    ) -> Result<R, AgentClientError>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let params = serde_json::to_value(params)
            .map_err(|error| AgentClientError::Protocol(error.to_string()))?;
        let result = self.request_value(method, params, timeout).await?;
        serde_json::from_value(result)
            .map_err(|error| AgentClientError::Protocol(error.to_string()))
    }

    pub async fn request_value(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, AgentClientError> {
        self.request_value_cancellable(method, params, timeout, None)
            .await
    }

    pub async fn request_value_cancellable(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<oneshot::Receiver<()>>,
    ) -> Result<Value, AgentClientError> {
        if timeout.is_zero() {
            return Err(AgentClientError::DeadlineExceeded);
        }
        let request_id = format!(
            "desktop-{}",
            self.inner.next_request_id.fetch_add(1, Ordering::Relaxed)
        );
        let timeout_ms = u64::try_from(timeout.as_millis().max(1)).unwrap_or(u64::MAX);
        let request = RequestEnvelope {
            request_id: request_id.clone(),
            method: method.into(),
            params,
            timeout_ms,
            protocol_version: PROTOCOL_VERSION,
        };
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self.inner.state.lock().expect("agent state lock poisoned");
            if let Some(error) = &state.terminal_error {
                return Err(error.clone());
            }
            state.pending.insert(request_id.clone(), sender);
        }
        let mut guard = PendingGuard::new(self.inner.state.clone(), request_id);

        if let Err(error) = self.inner.transport.send(&request).await {
            let error = AgentClientError::from(error);
            fail_all(&self.inner.state, error.clone());
            return Err(error);
        }
        let cancel_wait = async move {
            match cancel {
                Some(receiver) => {
                    let _ = receiver.await;
                }
                None => pending::<()>().await,
            }
        };
        let response = tokio::select! {
            response = receiver => {
                response
                    .map_err(|_| AgentClientError::TransportLost("response waiter dropped".into()))??
            }
            _ = tokio::time::sleep(timeout) => return Err(AgentClientError::DeadlineExceeded),
            _ = cancel_wait => return Err(AgentClientError::Cancelled),
        };
        guard.disarm();
        match response.payload {
            ResponsePayload::Success { result } => Ok(result),
            ResponsePayload::Failure { error } => Err(AgentClientError::Remote(error)),
        }
    }

    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("agent state lock poisoned")
            .pending
            .len()
    }
}

async fn reader_loop(transport: Arc<dyn AgentTransport>, state: SharedState) {
    loop {
        match transport.receive().await {
            Ok(response) => {
                let sender = state
                    .lock()
                    .expect("agent state lock poisoned")
                    .pending
                    .remove(&response.request_id);
                if let Some(sender) = sender {
                    let _ = sender.send(Ok(response));
                }
            }
            Err(error) => {
                fail_all(&state, AgentClientError::from(error));
                return;
            }
        }
    }
}

fn fail_all(state: &SharedState, error: AgentClientError) {
    let waiters = {
        let mut state = state.lock().expect("agent state lock poisoned");
        state.terminal_error = Some(error.clone());
        std::mem::take(&mut state.pending)
    };
    for (_, sender) in waiters {
        let _ = sender.send(Err(error.clone()));
    }
}

struct PendingGuard {
    state: SharedState,
    request_id: Option<String>,
}

impl PendingGuard {
    fn new(state: SharedState, request_id: String) -> Self {
        Self {
            state,
            request_id: Some(request_id),
        }
    }

    fn disarm(&mut self) {
        self.request_id = None;
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Some(request_id) = self.request_id.take() {
            self.state
                .lock()
                .expect("agent state lock poisoned")
                .pending
                .remove(&request_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::{Mutex as AsyncMutex, Notify};

    use super::*;

    #[derive(Default)]
    struct MockTransport {
        sent: AsyncMutex<Vec<RequestEnvelope>>,
        responses: AsyncMutex<VecDeque<Result<ResponseEnvelope, AgentTransportError>>>,
        response_ready: Notify,
        request_sent: Notify,
    }

    impl MockTransport {
        async fn wait_for_sent(&self, count: usize) {
            loop {
                if self.sent.lock().await.len() >= count {
                    return;
                }
                self.request_sent.notified().await;
            }
        }

        async fn respond(&self, response: Result<ResponseEnvelope, AgentTransportError>) {
            self.responses.lock().await.push_back(response);
            self.response_ready.notify_one();
        }

        async fn sent_requests(&self) -> Vec<RequestEnvelope> {
            self.sent.lock().await.clone()
        }
    }

    #[async_trait]
    impl AgentTransport for MockTransport {
        async fn send(&self, request: &RequestEnvelope) -> Result<(), AgentTransportError> {
            self.sent.lock().await.push(request.clone());
            self.request_sent.notify_one();
            Ok(())
        }

        async fn receive(&self) -> Result<ResponseEnvelope, AgentTransportError> {
            loop {
                if let Some(response) = self.responses.lock().await.pop_front() {
                    return response;
                }
                self.response_ready.notified().await;
            }
        }
    }

    #[tokio::test]
    async fn multiplexes_out_of_order_responses() {
        let transport = Arc::new(MockTransport::default());
        let client = AgentClient::new(transport.clone());
        let first_client = client.clone();
        let first = tokio::spawn(async move {
            first_client
                .request_value("test.first", json!({}), Duration::from_secs(1))
                .await
        });
        let second_client = client.clone();
        let second = tokio::spawn(async move {
            second_client
                .request_value("test.second", json!({}), Duration::from_secs(1))
                .await
        });
        transport.wait_for_sent(2).await;
        let requests = transport.sent_requests().await;
        transport
            .respond(Ok(ResponseEnvelope::success(
                requests[1].request_id.clone(),
                json!({ "order": 2 }),
            )))
            .await;
        transport
            .respond(Ok(ResponseEnvelope::success(
                requests[0].request_id.clone(),
                json!({ "order": 1 }),
            )))
            .await;

        assert_eq!(first.await.unwrap().unwrap()["order"], 1);
        assert_eq!(second.await.unwrap().unwrap()["order"], 2);
        assert_eq!(client.pending_count(), 0);
    }

    #[tokio::test]
    async fn deadline_and_cancellation_remove_pending_waiters() {
        let transport = Arc::new(MockTransport::default());
        let client = AgentClient::new(transport.clone());
        let error = client
            .request_value("test.timeout", json!({}), Duration::from_millis(10))
            .await
            .unwrap_err();
        assert_eq!(error, AgentClientError::DeadlineExceeded);
        assert_eq!(client.pending_count(), 0);

        let request_client = client.clone();
        let (cancel_sender, cancel_receiver) = oneshot::channel();
        let request = tokio::spawn(async move {
            request_client
                .request_value_cancellable(
                    "test.cancel",
                    json!({}),
                    Duration::from_secs(10),
                    Some(cancel_receiver),
                )
                .await
        });
        transport.wait_for_sent(2).await;
        cancel_sender.send(()).unwrap();
        assert_eq!(request.await.unwrap(), Err(AgentClientError::Cancelled));
        assert_eq!(client.pending_count(), 0);
    }

    #[tokio::test]
    async fn disconnect_wakes_every_pending_request() {
        let transport = Arc::new(MockTransport::default());
        let client = AgentClient::new(transport.clone());
        let one_client = client.clone();
        let one = tokio::spawn(async move {
            one_client
                .request_value("test.one", json!({}), Duration::from_secs(1))
                .await
        });
        let two_client = client.clone();
        let two = tokio::spawn(async move {
            two_client
                .request_value("test.two", json!({}), Duration::from_secs(1))
                .await
        });
        transport.wait_for_sent(2).await;
        transport.respond(Err(AgentTransportError::Closed)).await;

        for result in [one.await.unwrap(), two.await.unwrap()] {
            assert!(matches!(result, Err(AgentClientError::TransportLost(_))));
        }
        assert_eq!(client.pending_count(), 0);

        let sent_before = transport.sent_requests().await.len();
        let error = client
            .request_value("test.after_disconnect", json!({}), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(error, AgentClientError::TransportLost(_)));
        assert_eq!(transport.sent_requests().await.len(), sent_before);
    }

    #[tokio::test]
    async fn explicit_disconnect_wakes_pending_without_waiting_for_socket_eof() {
        let transport = Arc::new(MockTransport::default());
        let client = AgentClient::new(transport.clone());
        let request_client = client.clone();
        let request = tokio::spawn(async move {
            request_client
                .request_value("test.pending", json!({}), Duration::from_secs(10))
                .await
        });
        transport.wait_for_sent(1).await;
        client.disconnect("device disconnected");
        let error = request.await.unwrap().unwrap_err();
        assert_eq!(
            error,
            AgentClientError::TransportLost("device disconnected".into())
        );
        assert_eq!(client.pending_count(), 0);
    }
}
