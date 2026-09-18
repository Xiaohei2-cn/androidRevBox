use std::io::{Error, ErrorKind};
use std::sync::Arc;
use std::time::Duration;

use agent_protocol::method::SYSTEM_HELLO;
use agent_protocol::{
    AgentError, ErrorCode, MAX_FRAME_SIZE, PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope,
    encode_json,
};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::router::Router;

pub async fn serve_connection<S>(stream: S, router: Arc<Router>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(writer));

    let hello = read_message::<_, RequestEnvelope>(&mut reader).await?;
    if hello.method != SYSTEM_HELLO {
        write_response(
            &writer,
            &ResponseEnvelope::failure(
                hello.request_id,
                AgentError::new(
                    ErrorCode::PermissionDenied,
                    "system.hello must be the first request",
                ),
            ),
        )
        .await?;
        return Ok(());
    }
    let hello_response = execute_request(&router, hello).await;
    let authenticated = matches!(
        &hello_response.payload,
        agent_protocol::ResponsePayload::Success { .. }
    );
    write_response(&writer, &hello_response).await?;
    if !authenticated {
        return Ok(());
    }

    let mut active = JoinSet::new();
    loop {
        let request = match read_message::<_, RequestEnvelope>(&mut reader).await {
            Ok(request) => request,
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(error) => {
                active.abort_all();
                while active.join_next().await.is_some() {}
                return Err(error);
            }
        };
        let router = router.clone();
        let writer = writer.clone();
        active.spawn(async move {
            let response = if request.method == SYSTEM_HELLO {
                ResponseEnvelope::failure(
                    request.request_id,
                    AgentError::new(
                        ErrorCode::InvalidRequest,
                        "system.hello is only allowed as the first request",
                    ),
                )
            } else {
                execute_request(&router, request).await
            };
            write_response(&writer, &response).await
        });
        while active.try_join_next().is_some() {}
    }

    active.abort_all();
    while active.join_next().await.is_some() {}
    Ok(())
}

async fn execute_request(router: &Router, request: RequestEnvelope) -> ResponseEnvelope {
    let request_id = request.request_id.clone();
    if request.protocol_version != PROTOCOL_VERSION {
        return ResponseEnvelope::failure(
            request_id,
            AgentError::new(
                ErrorCode::IncompatibleVersion,
                format!(
                    "request protocol version {} is incompatible with agent version {}",
                    request.protocol_version, PROTOCOL_VERSION
                ),
            ),
        );
    }
    if request.timeout_ms == 0 {
        return ResponseEnvelope::failure(
            request_id,
            AgentError::new(
                ErrorCode::InvalidRequest,
                "timeout_ms must be greater than zero",
            ),
        );
    }
    match tokio::time::timeout(
        Duration::from_millis(request.timeout_ms),
        router.dispatch(&request.method, request.params),
    )
    .await
    {
        Ok(Ok(result)) => ResponseEnvelope::success(request_id, result),
        Ok(Err(error)) => ResponseEnvelope::failure(request_id, error),
        Err(_) => ResponseEnvelope::failure(
            request_id,
            AgentError::new(ErrorCode::DeadlineExceeded, "request deadline exceeded"),
        ),
    }
}

async fn write_response<W>(
    writer: &Arc<Mutex<W>>,
    response: &ResponseEnvelope,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let frame = encode_json(response).map_err(|error| Error::new(ErrorKind::InvalidData, error))?;
    writer.lock().await.write_all(&frame).await
}

pub async fn read_message<R, T>(reader: &mut R) -> std::io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_FRAME_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "frame exceeds protocol limit",
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).map_err(|error| Error::new(ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_protocol::method::{SYSTEM_HEALTH, SYSTEM_HELLO};
    use agent_protocol::{
        EmptyParams, HelloParams, PermissionInfo, ProviderHealth, ProviderInfo, RequestEnvelope,
        ResponseEnvelope, ResponsePayload,
    };
    use serde_json::Value;
    use tokio::io::{AsyncWriteExt, DuplexStream};

    use crate::provider::{Provider, ProviderFuture, RequestContext};
    use crate::system_router;

    use super::*;

    fn request(method: &str, params: serde_json::Value) -> RequestEnvelope {
        request_with_timeout(method, params, 1_000)
    }

    fn request_with_timeout(
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> RequestEnvelope {
        RequestEnvelope {
            request_id: format!("req-{method}"),
            method: method.into(),
            params,
            timeout_ms,
            protocol_version: PROTOCOL_VERSION,
        }
    }

    async fn write_message(stream: &mut DuplexStream, request: &RequestEnvelope) {
        stream
            .write_all(&encode_json(request).unwrap())
            .await
            .unwrap();
    }

    fn router() -> Arc<Router> {
        Arc::new(system_router(
            "secret",
            PermissionInfo {
                shell: true,
                root: false,
                selinux_enforcing: false,
            },
        ))
    }

    struct DelayProvider;

    impl Provider for DelayProvider {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                name: "delay-test".into(),
                version: "1.0.0".into(),
                health: ProviderHealth::Ready,
                required_permissions: Vec::new(),
                last_error: None,
            }
        }

        fn methods(&self) -> &'static [&'static str] {
            &["test.slow", "test.fast"]
        }

        fn handle<'a>(
            &'a self,
            _context: RequestContext,
            method: &'a str,
            _params: Value,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                if method == "test.slow" {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Ok(serde_json::json!({ "method": method }))
            })
        }
    }

    fn router_with_delay_provider() -> Arc<Router> {
        let mut router = system_router(
            "secret",
            PermissionInfo {
                shell: true,
                root: false,
                selinux_enforcing: false,
            },
        );
        router.register(Arc::new(DelayProvider)).unwrap();
        Arc::new(router)
    }

    async fn authenticate(client: &mut DuplexStream) {
        write_message(
            client,
            &request(
                SYSTEM_HELLO,
                serde_json::to_value(HelloParams::v1("secret", "desktop-test")).unwrap(),
            ),
        )
        .await;
        let response: ResponseEnvelope = read_message(client).await.unwrap();
        assert!(matches!(response.payload, ResponsePayload::Success { .. }));
    }

    #[tokio::test]
    async fn requires_authenticated_hello_before_other_methods() {
        let (mut client, server) = tokio::io::duplex(8 * 1024);
        let task = tokio::spawn(serve_connection(server, router()));
        write_message(
            &mut client,
            &request(SYSTEM_HEALTH, serde_json::json!(EmptyParams {})),
        )
        .await;
        let response: ResponseEnvelope = read_message(&mut client).await.unwrap();
        let ResponsePayload::Failure { error } = response.payload else {
            panic!("expected authentication error");
        };
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        drop(client);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn hello_then_health_share_one_connection() {
        let (mut client, server) = tokio::io::duplex(8 * 1024);
        let task = tokio::spawn(serve_connection(server, router()));
        authenticate(&mut client).await;

        write_message(
            &mut client,
            &request(SYSTEM_HEALTH, serde_json::json!(EmptyParams {})),
        )
        .await;
        let health: ResponseEnvelope = read_message(&mut client).await.unwrap();
        let ResponsePayload::Success { result } = health.payload else {
            panic!("expected health result");
        };
        assert_eq!(result["status"], "ready");
        drop(client);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn concurrent_requests_can_respond_out_of_order() {
        let (mut client, server) = tokio::io::duplex(8 * 1024);
        let task = tokio::spawn(serve_connection(server, router_with_delay_provider()));
        authenticate(&mut client).await;

        write_message(&mut client, &request("test.slow", serde_json::json!({}))).await;
        write_message(&mut client, &request("test.fast", serde_json::json!({}))).await;

        let first: ResponseEnvelope = read_message(&mut client).await.unwrap();
        let second: ResponseEnvelope = read_message(&mut client).await.unwrap();
        assert_eq!(first.request_id, "req-test.fast");
        assert_eq!(second.request_id, "req-test.slow");
        drop(client);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn provider_deadline_returns_structured_error() {
        let (mut client, server) = tokio::io::duplex(8 * 1024);
        let task = tokio::spawn(serve_connection(server, router_with_delay_provider()));
        authenticate(&mut client).await;

        write_message(
            &mut client,
            &request_with_timeout("test.slow", serde_json::json!({}), 10),
        )
        .await;
        let response: ResponseEnvelope = read_message(&mut client).await.unwrap();
        let ResponsePayload::Failure { error } = response.payload else {
            panic!("expected deadline error");
        };
        assert_eq!(error.code, ErrorCode::DeadlineExceeded);
        drop(client);
        task.await.unwrap().unwrap();
    }
}
