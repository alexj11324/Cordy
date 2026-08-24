use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cordy_auth::daemon_token_cache::{DaemonTokenCache, DaemonTokenIdentity};
use cordy_auth::membership_cache::MembershipCache;
use cordy_auth::pat_cache::PatCache;

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
async fn auth_caches_round_trip_expire_and_invalidate() -> anyhow::Result<()> {
    let Some(server) = RedisServer::start().await? else {
        eprintln!("redis-server is unavailable; skipping Redis integration test");
        return Ok(());
    };
    let client = redis::Client::open(server.url.as_str())?;
    let pat = PatCache::new(client.clone()).await?;
    let daemon = DaemonTokenCache::new(client.clone()).await?;
    let membership = MembershipCache::new(client.clone()).await?;

    assert_eq!(pat.get("missing").await, None);
    pat.set("pat-hash", "user-a", 5).await;
    assert_eq!(pat.get("pat-hash").await.as_deref(), Some("user-a"));
    let mut connection = client.get_multiplexed_async_connection().await?;
    let pat_ttl: i64 = redis::cmd("TTL")
        .arg("mul:auth:pat:pat-hash")
        .query_async(&mut connection)
        .await?;
    assert!(pat_ttl > 0 && pat_ttl <= 5);
    pat.invalidate("pat-hash").await;
    assert_eq!(pat.get("pat-hash").await, None);
    pat.set("zero-ttl", "user-a", 0).await;
    assert_eq!(pat.get("zero-ttl").await, None);

    let identity = DaemonTokenIdentity {
        workspace_id: "workspace-a".into(),
        daemon_id: "daemon-a".into(),
    };
    daemon.set("daemon-hash", &identity, 5).await;
    assert_eq!(daemon.get("daemon-hash").await, Some(identity));
    daemon.invalidate("daemon-hash").await;
    assert_eq!(daemon.get("daemon-hash").await, None);
    redis::cmd("SET")
        .arg("mul:auth:daemon:malformed")
        .arg("not-json")
        .query_async::<()>(&mut connection)
        .await?;
    assert_eq!(daemon.get("malformed").await, None);

    membership.set("user-a", "workspace-a").await;
    membership.set("user-b", "workspace-a").await;
    assert!(membership.get("user-a", "workspace-a").await);
    assert!(membership.get("user-b", "workspace-a").await);
    membership.invalidate("user-a", "workspace-a").await;
    assert!(!membership.get("user-a", "workspace-a").await);
    assert!(membership.get("user-b", "workspace-a").await);

    let ttl: i64 = redis::cmd("TTL")
        .arg("mul:auth:member:user-b:workspace-a")
        .query_async(&mut connection)
        .await?;
    assert!(ttl > 0 && ttl <= 5 * 60);
    Ok(())
}
