//! Clerk session verification and profile resolution for the public session
//! exchange endpoint.

use async_trait::async_trait;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClerkIdentity {
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClerkAuthError {
    Invalid,
    Unavailable,
}

#[async_trait]
pub trait ClerkSessionVerifier: Send + Sync {
    async fn verify(&self, token: &str) -> Result<ClerkIdentity, ClerkAuthError>;
}

pub struct ClerkAuthClient {
    http: reqwest::Client,
    secret_key: String,
    decoding_key: DecodingKey,
    validation: Validation,
    authorized_parties: Vec<String>,
}

impl ClerkAuthClient {
    pub fn from_config(config: &patchbay_config::AuthConfig) -> anyhow::Result<Option<Self>> {
        let secret_key = trimmed(config.clerk_secret_key.as_deref());
        let jwt_key = trimmed(config.clerk_jwt_key.as_deref());
        let issuer = trimmed(config.clerk_issuer.as_deref());
        let authorized_parties = split_origins(config.clerk_authorized_parties.as_deref())?;

        if secret_key.is_empty()
            && jwt_key.is_empty()
            && issuer.is_empty()
            && authorized_parties.is_empty()
        {
            return Ok(None);
        }
        anyhow::ensure!(
            !secret_key.is_empty()
                && !jwt_key.is_empty()
                && !issuer.is_empty()
                && !authorized_parties.is_empty(),
            "CLERK_SECRET_KEY, CLERK_JWT_KEY, CLERK_ISSUER, and CLERK_AUTHORIZED_PARTIES must be configured together"
        );

        let normalized_pem = jwt_key.replace("\\n", "\n");
        let decoding_key =
            DecodingKey::from_rsa_pem(normalized_pem.as_bytes()).map_err(|error| {
                anyhow::anyhow!("CLERK_JWT_KEY is not a valid RSA public key: {error}")
            })?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_nbf = true;
        validation.set_required_spec_claims(&["exp", "nbf", "iss", "sub"]);
        validation.set_issuer(&[issuer.trim_end_matches('/')]);
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        Ok(Some(Self {
            http,
            secret_key,
            decoding_key,
            validation,
            authorized_parties,
        }))
    }

    fn verify_claims(&self, token: &str) -> Result<ClerkClaims, ClerkAuthError> {
        let claims = decode::<ClerkClaims>(token, &self.decoding_key, &self.validation)
            .map_err(|_| ClerkAuthError::Invalid)?
            .claims;
        if claims.sub.trim().is_empty()
            || claims.sts.as_deref() == Some("pending")
            || !is_authorized_party(&self.authorized_parties, claims.azp.as_deref())
        {
            return Err(ClerkAuthError::Invalid);
        }
        Ok(claims)
    }

    async fn fetch_identity(&self, user_id: &str) -> Result<ClerkIdentity, ClerkAuthError> {
        let mut url = url::Url::parse("https://api.clerk.com/v1/users/")
            .map_err(|_| ClerkAuthError::Unavailable)?;
        url.path_segments_mut()
            .map_err(|_| ClerkAuthError::Unavailable)?
            .push(user_id);
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.secret_key)
            .send()
            .await
            .map_err(|_| ClerkAuthError::Unavailable)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ClerkAuthError::Invalid);
        }
        if !response.status().is_success() {
            return Err(ClerkAuthError::Unavailable);
        }
        let user = response
            .json::<ClerkUser>()
            .await
            .map_err(|_| ClerkAuthError::Unavailable)?;
        if user.banned || user.locked {
            return Err(ClerkAuthError::Invalid);
        }
        let primary_id = user
            .primary_email_address_id
            .as_deref()
            .ok_or(ClerkAuthError::Invalid)?;
        let email = user
            .email_addresses
            .iter()
            .find(|email| email.id == primary_id)
            .filter(|email| {
                email
                    .verification
                    .as_ref()
                    .is_some_and(|verification| verification.status == "verified")
            })
            .map(|email| email.email_address.trim().to_lowercase())
            .filter(|email| !email.is_empty())
            .ok_or(ClerkAuthError::Invalid)?;
        let name = [user.first_name.as_deref(), user.last_name.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let name = if name.is_empty() {
            user.username
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    email
                        .split_once('@')
                        .map_or(email.as_str(), |(name, _)| name)
                })
                .to_string()
        } else {
            name
        };
        Ok(ClerkIdentity {
            email,
            name,
            avatar_url: user
                .image_url
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }
}

#[async_trait]
impl ClerkSessionVerifier for ClerkAuthClient {
    async fn verify(&self, token: &str) -> Result<ClerkIdentity, ClerkAuthError> {
        let claims = self.verify_claims(token)?;
        self.fetch_identity(&claims.sub).await
    }
}

#[derive(Debug, Deserialize)]
struct ClerkClaims {
    sub: String,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    sts: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClerkUser {
    #[serde(default)]
    banned: bool,
    #[serde(default)]
    locked: bool,
    primary_email_address_id: Option<String>,
    #[serde(default)]
    email_addresses: Vec<ClerkEmailAddress>,
    first_name: Option<String>,
    last_name: Option<String>,
    username: Option<String>,
    image_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClerkEmailAddress {
    id: String,
    email_address: String,
    verification: Option<ClerkEmailVerification>,
}

#[derive(Debug, Deserialize)]
struct ClerkEmailVerification {
    status: String,
}

fn trimmed(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_string()
}

fn split_origins(value: Option<&str>) -> anyhow::Result<Vec<String>> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let url = url::Url::parse(value)
                .map_err(|_| anyhow::anyhow!("CLERK_AUTHORIZED_PARTIES contains an invalid URL"))?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "CLERK_AUTHORIZED_PARTIES entries must be HTTP(S) origins"
            );
            Ok(url.origin().ascii_serialization())
        })
        .collect()
}

fn is_authorized_party(authorized_parties: &[String], party: Option<&str>) -> bool {
    party.is_some_and(|party| {
        authorized_parties
            .iter()
            .any(|allowed| allowed == party.trim_end_matches('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_configuration_fails_closed() {
        let config = patchbay_config::AuthConfig {
            clerk_secret_key: Some("sk_test_example".into()),
            ..Default::default()
        };
        assert!(ClerkAuthClient::from_config(&config).is_err());
    }

    #[test]
    fn authorized_parties_must_be_origins() {
        assert!(split_origins(Some("https://app.example.com/path")).is_err());
        assert_eq!(
            split_origins(Some("https://app.example.com, https://local.example.com")).unwrap(),
            vec!["https://app.example.com", "https://local.example.com"]
        );
    }

    #[test]
    fn missing_or_unknown_authorized_party_is_rejected() {
        let allowed = vec!["https://app.example.com".to_string()];
        assert!(!is_authorized_party(&allowed, None));
        assert!(!is_authorized_party(&allowed, Some("https://evil.example")));
        assert!(is_authorized_party(
            &allowed,
            Some("https://app.example.com/")
        ));
    }
}
