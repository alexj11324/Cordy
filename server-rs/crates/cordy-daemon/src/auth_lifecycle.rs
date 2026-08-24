//! Owned daemon PAT-renewal lifecycle.
//!
//! Renewal is deliberately daemon-owned rather than provider-owned: it runs
//! before the first workspace API call and then on the same long cadence as
//! Go. A renewal failure never blocks startup or stops the loop; the following
//! workspace sync remains the authoritative authentication/readiness check.

use std::sync::Arc;
use std::time::Duration;

use crate::client::{request_status_code, Client};
use crate::repocache::Ctx;

pub const TOKEN_RENEWAL_INTERVAL: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const TOKEN_RENEWAL_TIMEOUT: Duration = Duration::from_secs(15);

/// One independently testable renewal attempt. Errors are classified and
/// logged without carrying the bearer token into diagnostics.
pub(crate) async fn renew_token_once(client: &Client, profile: &str, ctx: &Ctx) {
    let result = tokio::time::timeout(TOKEN_RENEWAL_TIMEOUT, client.renew_token(ctx)).await;
    match result {
        Ok(Ok(response)) if response.renewed => {
            tracing::info!(expires_at = %response.expires_at, "auth token renewed");
        }
        Ok(Ok(response)) => {
            tracing::debug!(expires_at = %response.expires_at, "auth token not yet eligible for renewal");
        }
        Ok(Err(error)) if request_status_code(&error) == Some(401) => {
            let login_hint = if profile.is_empty() {
                "'cordy login'".to_string()
            } else {
                format!("'cordy login --profile {profile}'")
            };
            tracing::warn!(%error, "auth token rejected by server — run {login_hint} to re-authenticate, then restart the daemon");
        }
        Ok(Err(error)) => {
            tracing::debug!(%error, "token renewal failed; will retry on next cycle");
        }
        Err(_) => {
            tracing::debug!(timeout = ?TOKEN_RENEWAL_TIMEOUT, "token renewal timed out; will retry on next cycle");
        }
    }
}

/// Long-running owner. The startup path calls [`renew_token_once`] first;
/// this loop waits one full cadence before its next request and exits only
/// when the daemon root is cancelled.
pub(crate) async fn token_renewal_loop(client: Arc<Client>, profile: String, ctx: Ctx) {
    token_renewal_loop_with_interval(client, profile, ctx, TOKEN_RENEWAL_INTERVAL).await;
}

async fn token_renewal_loop_with_interval(
    client: Arc<Client>,
    profile: String,
    ctx: Ctx,
    interval: Duration,
) {
    loop {
        tokio::select! {
            () = ctx.cancelled() => return,
            _ = tokio::time::sleep(interval) => {
                renew_token_once(&client, &profile, &ctx).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repocache::CancelCause;

    #[tokio::test]
    async fn renewal_owner_obeys_root_cancellation_without_an_initial_request() {
        let client = Arc::new(Client::new("http://127.0.0.1:1"));
        let ctx = Ctx::new();
        let owner = tokio::spawn(token_renewal_loop_with_interval(
            client,
            String::new(),
            ctx.clone(),
            Duration::from_secs(60),
        ));

        ctx.cancel_with(CancelCause::Shutdown);
        tokio::task::yield_now().await;
        assert!(owner.await.is_ok());
    }
}
