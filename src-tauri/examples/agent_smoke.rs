use std::env;
use std::error::Error;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_protocol::HealthStatus;
use app_reverse_tools_lib::agent_client::AgentClient;
use app_reverse_tools_lib::agent_transport::FramedAgentTransport;
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let address: SocketAddr = arguments
        .next()
        .ok_or_else(|| IoError::new(ErrorKind::InvalidInput, "missing address"))?
        .parse()?;
    if !address.ip().is_loopback() {
        return Err(IoError::new(ErrorKind::InvalidInput, "address must be loopback").into());
    }
    let token_file = arguments
        .next()
        .ok_or_else(|| IoError::new(ErrorKind::InvalidInput, "missing token file"))?;
    let health_count = arguments
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(1);
    if !(1..=1_000_000).contains(&health_count) {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "health count must be between 1 and 1000000",
        )
        .into());
    }
    if arguments.next().is_some() {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "usage: agent_smoke ADDRESS TOKEN_FILE [HEALTH_COUNT]",
        )
        .into());
    }
    let auth_token = fs::read_to_string(token_file)?;
    let stream = TcpStream::connect(address).await?;
    let client = AgentClient::new(Arc::new(FramedAgentTransport::new(stream)));
    let hello = client
        .hello(
            auth_token.trim_end_matches(['\r', '\n']),
            env!("CARGO_PKG_VERSION"),
            Duration::from_secs(5),
        )
        .await?;
    let started_at = Instant::now();
    let mut latencies_us = Vec::with_capacity(health_count);
    let mut last_health = None;
    for _ in 0..health_count {
        let request_started_at = Instant::now();
        let health = client.health(Duration::from_secs(5)).await?;
        if health.status != HealthStatus::Ready {
            return Err(IoError::other("agent health is not ready").into());
        }
        latencies_us
            .push(u64::try_from(request_started_at.elapsed().as_micros()).unwrap_or(u64::MAX));
        last_health = Some(health);
    }
    latencies_us.sort_unstable();
    let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    let percentile = |percent: usize| {
        let index = (latencies_us.len() * percent)
            .div_ceil(100)
            .saturating_sub(1);
        latencies_us[index]
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "protocol_version": hello.protocol_version,
            "agent_version": hello.agent_version,
            "permissions": hello.permissions,
            "providers": hello.providers,
            "capabilities": hello.capabilities,
            "health": last_health.expect("health count is nonzero"),
            "health_metrics": {
                "count": health_count,
                "elapsed_ms": elapsed_ms,
                "p50_us": percentile(50),
                "p95_us": percentile(95),
                "max_us": latencies_us[latencies_us.len() - 1],
            },
        }))?
    );
    Ok(())
}
