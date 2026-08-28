use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use patchbay_service::empty_claim_cache::EmptyClaimCache;

struct RedisServer {
    child: Child,
    url: String,
}

impl RedisServer {
    async fn start() -> anyhow::Result<Option<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let port_string = port.to_string();
        drop(listener);
        let child = match Command::new("redis-server")
            .args([
                "--bind",
                "127.0.0.1",
                "--port",
                &port_string,
                "--save",
                "",
                "--appendonly",
                "no",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let server = Self {
            child,
            url: format!("redis://127.0.0.1:{port}/"),
        };
        let client = redis::Client::open(server.url.as_str())?;
        for _ in 0..50 {
            if let Ok(mut connection) = client.get_multiplexed_async_connection().await {
                if redis::cmd("PING")
                    .query_async::<()>(&mut connection)
                    .await
                    .is_ok()
                {
                    return Ok(Some(server));
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::bail!("redis-server did not become ready")
    }
}

impl Drop for RedisServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn empty_claim_cache_rejects_stale_verdict_after_bump() -> anyhow::Result<()> {
    let Some(server) = RedisServer::start().await? else {
        eprintln!("redis-server is unavailable; skipping Redis integration test");
        return Ok(());
    };
    let client = redis::Client::open(server.url.as_str())?;
    let cache = EmptyClaimCache::new(patchbay_redis::RecoveringConnection::new(client));
    let runtime_id = "runtime-a";

    let observed = cache.current_version(runtime_id).await;
    assert_eq!(observed, 0);
    cache.mark_empty(runtime_id, observed).await;
    assert!(cache.is_empty(runtime_id).await);

    cache.bump(runtime_id).await;
    assert!(!cache.is_empty(runtime_id).await);
    assert_eq!(cache.current_version(runtime_id).await, 1);

    // Reproduce the slow-claim race: a SELECT observed v1, an enqueue bumps
    // to v2, then the stale SELECT writes its v1 verdict.
    let stale_version = cache.current_version(runtime_id).await;
    cache.bump(runtime_id).await;
    cache.mark_empty(runtime_id, stale_version).await;
    assert!(!cache.is_empty(runtime_id).await);
    Ok(())
}

#[tokio::test]
async fn disabled_empty_claim_cache_is_a_no_op() {
    let cache = EmptyClaimCache::disabled();
    assert_eq!(cache.current_version("runtime").await, 0);
    assert!(!cache.is_empty("runtime").await);
    cache.mark_empty("runtime", 0).await;
    cache.bump("runtime").await;
}
