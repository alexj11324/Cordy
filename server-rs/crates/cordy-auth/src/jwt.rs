//! JWT secret management and token minting — port of
//! `server/internal/auth/jwt.go`.

use std::sync::OnceLock;

use rand::RngCore;
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
}
