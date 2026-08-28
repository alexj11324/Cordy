//! JWT secret management and token minting.
use std::sync::OnceLock;

use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};

const DEFAULT_JWT_SECRET: &str = "cordy-dev-secret-change-in-production";

static JWT_SECRET: OnceLock<String> = OnceLock::new();

/// Installs the effective server configuration before any token is decoded or
/// minted. This keeps TOML-backed configuration on the same singleton path as
/// `JWT_SECRET` instead of silently falling back to the development key.
pub fn configure_jwt_secret(secret: Option<&str>) -> anyhow::Result<()> {
    let effective = secret
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_JWT_SECRET)
        .to_string();
    JWT_SECRET
        .set(effective)
        .map_err(|_| anyhow::anyhow!("JWT secret was already initialized"))
}

/// Process-wide HS256 signing secret. Reads `JWT_SECRET` on first call and
/// falls back to a dev-only default — mirrors Go's sync.Once singleton.
pub fn jwt_secret() -> &'static str {
    JWT_SECRET.get_or_init(|| {
        std::env::var("JWT_SECRET")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_JWT_SECRET.to_string())
    })
}

/// Returned by [`validate_jwt_secret`] when the configured secret is empty or
/// one of the publicly known defaults shipped in repo/config templates.
pub const ERR_INSECURE_JWT_SECRET: &str = "JWT_SECRET is empty or a known insecure default";

/// Values that must never sign production tokens. Add any placeholder a
/// template ships with to this list before publishing it.
fn is_insecure_jwt_secret(secret: &str) -> bool {
    secret.is_empty() || secret == DEFAULT_JWT_SECRET || secret == "change-me-in-production"
}

/// Reports whether `secret` is safe to sign production tokens with.
/// Deliberately cheap and side-effect free so the boot path and tests can
/// call it directly.
pub fn validate_jwt_secret(secret: &str) -> Result<(), &'static str> {
    if is_insecure_jwt_secret(secret.trim()) {
        return Err(ERR_INSECURE_JWT_SECRET);
    }
    Ok(())
}

/// Personal access token: `mul_` + 40 random hex chars.
pub fn generate_pat_token() -> anyhow::Result<String> {
    Ok(format!("mul_{}", random_hex20()?))
}

/// Daemon auth token: `mdt_` + 40 random hex chars.
pub fn generate_daemon_token() -> anyhow::Result<String> {
    Ok(format!("mdt_{}", random_hex20()?))
}

/// Task-scoped agent token: `mat_` + 40 random hex chars. Bound to a specific
/// (agent_id, task_id) pair server-side; injected by the daemon into the agent
/// process in place of its owner PAT (MUL-2600).
pub fn generate_agent_task_token() -> anyhow::Result<String> {
    Ok(format!("mat_{}", random_hex20()?))
}

/// Hex-encoded SHA-256 of a token string — the DB stores this, never the raw token.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[derive(Serialize)]
struct UserClaims<'a> {
    sub: &'a str,
    email: &'a str,
    name: &'a str,
    exp: i64,
    iat: i64,
}

/// Issues the authenticated user JWT used by browser sessions and the CLI
/// handoff endpoint. Claim names and HS256 signing match the Go handler.
pub fn issue_user_jwt(user_id: &str, email: &str, name: &str) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp();
    issue_user_jwt_at(
        user_id,
        email,
        name,
        now,
        crate::cookie::auth_token_ttl(),
        jwt_secret(),
    )
}

fn issue_user_jwt_at(
    user_id: &str,
    email: &str,
    name: &str,
    now: i64,
    ttl: i64,
    secret: &str,
) -> anyhow::Result<String> {
    Ok(jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &UserClaims {
            sub: user_id,
            email,
            name,
            exp: now
                .checked_add(ttl)
                .ok_or_else(|| anyhow::anyhow!("JWT expiration overflow"))?,
            iat: now,
        },
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )?)
}

fn random_hex20() -> anyhow::Result<String> {
    let mut b = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut b);
    Ok(hex::encode(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_known_insecure_defaults() {
        assert!(validate_jwt_secret("").is_err());
        assert!(validate_jwt_secret("   ").is_err());
        assert!(validate_jwt_secret(DEFAULT_JWT_SECRET).is_err());
        assert!(validate_jwt_secret("change-me-in-production").is_err());
        assert!(validate_jwt_secret("a-long-random-production-secret").is_ok());
    }

    #[test]
    fn pat_token_shape() {
        let t = generate_pat_token().unwrap();
        assert!(t.starts_with("mul_"));
        assert_eq!(t.len(), 4 + 40);
        assert!(t[4..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn daemon_and_task_token_shapes() {
        let d = generate_daemon_token().unwrap();
        assert!(d.starts_with("mdt_") && d.len() == 44);
        let a = generate_agent_task_token().unwrap();
        assert!(a.starts_with("mat_") && a.len() == 44);
    }

    #[test]
    fn hash_token_matches_sha256_known_vector() {
        // sha256("abc")
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn user_jwt_claims_and_expiry_match_go_contract() {
        let token =
            issue_user_jwt_at("user-1", "a@example.com", "Alex", 100, 300, "secret").unwrap();
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.validate_exp = false;
        let claims = jsonwebtoken::decode::<serde_json::Value>(
            &token,
            &jsonwebtoken::DecodingKey::from_secret(b"secret"),
            &validation,
        )
        .unwrap()
        .claims;
        assert_eq!(claims["sub"], "user-1");
        assert_eq!(claims["email"], "a@example.com");
        assert_eq!(claims["name"], "Alex");
        assert_eq!(claims["iat"], 100);
        assert_eq!(claims["exp"], 400);
    }

    #[test]
    fn user_jwt_rejects_expiration_overflow() {
        let error = issue_user_jwt_at(
            "user-1",
            "a@example.com",
            "Alex",
            i64::MAX - 1,
            10,
            "secret",
        )
        .expect_err("overflowing exp must fail");
        assert!(
            error.to_string().contains("overflow"),
            "unexpected error: {error}"
        );
    }
}
