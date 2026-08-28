//! Operator opt-in that lets a Plugin author point an `mcp` hook at a server
//! running on their own machine.
//!
//! With no entries in the
//! environment every check below returns false and the original validation
//! runs unmodified — which is every production deployment.

use std::sync::Arc;

use rustls::ClientConfig;
use url::Url;

/// Read by both the server and the daemon. They are separate processes, so
/// the value is looked up per call rather than cached at init.
pub const DEV_ORIGINS_ENV: &str = "CORDY_PLUGIN_DEV_ORIGINS";

/// CA bundle to trust for dev origins. A locally-run MCP server still has to
/// speak HTTPS, so the dev allowance is "trust this extra CA", never "skip
/// verification".
pub const DEV_CA_ENV: &str = "CORDY_PLUGIN_DEV_CA";

/// Reports whether the operator named this exact origin.
///
/// Exact match on scheme + host + port. A prefix or suffix match would let
/// `http://127.0.0.1:9000` authorise `http://127.0.0.1:9000.example.com`.
pub fn is_dev_origin(endpoint: &Url) -> bool {
    let configured = std::env::var(DEV_ORIGINS_ENV).unwrap_or_default();
    is_dev_origin_configured(endpoint, &configured)
}

/// Testable core of [`is_dev_origin`]: identical matching rules against an
/// explicit configuration string.
pub fn is_dev_origin_configured(endpoint: &Url, configured: &str) -> bool {
    if endpoint.host_str().is_none_or(str::is_empty) {
        return false;
    }
    let configured = configured.trim();
    if configured.is_empty() {
        return false;
    }
    let mut origin = format!(
        "{}://{}",
        endpoint.scheme(),
        endpoint.host_str().unwrap_or_default()
    );
    if let Some(port) = endpoint.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    configured.split(',').any(|entry| entry.trim() == origin)
}

/// TLS config for a dev origin, or `None` to leave the transport's default
/// alone.
///
/// `None` is also what an unreadable or unparseable CA file yields: failing
/// closed here means a mistyped path produces a certificate error, not a
/// silently unverified connection.
pub fn dev_tls_config() -> Option<Arc<ClientConfig>> {
    let path = std::env::var(DEV_CA_ENV).unwrap_or_default();
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    dev_tls_config_from_path(path)
}

pub(crate) fn dev_tls_config_from_path(path: &str) -> Option<Arc<ClientConfig>> {
    let pem = std::fs::read(path).ok()?;
    let mut reader = std::io::BufReader::new(&pem[..]);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in certs {
        // Individual parse failures are skipped; an empty store below fails
        // closed instead of silently trusting nothing new.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return None;
    }
    // Same provider pin as client.rs: two providers in the graph make the
    // process-default lookup ambiguous.
    let mut config = ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("default protocol versions are always supported")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Some(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn dev_origin_requires_exact_scheme_host_port_match() {
        let endpoint = url("http://127.0.0.1:9000");
        assert!(is_dev_origin_configured(&endpoint, "http://127.0.0.1:9000"));
        assert!(is_dev_origin_configured(
            &endpoint,
            " http://127.0.0.1:9000 ,https://x.test"
        ));
        assert!(!is_dev_origin_configured(&endpoint, ""));
        assert!(!is_dev_origin_configured(&endpoint, "   "));
        assert!(!is_dev_origin_configured(
            &endpoint,
            "http://127.0.0.1:9000.example.com"
        ));
        assert!(!is_dev_origin_configured(&endpoint, "http://127.0.0.1"));
        assert!(!is_dev_origin_configured(
            &endpoint,
            "https://127.0.0.1:9000"
        ));
        assert!(!is_dev_origin_configured(
            &endpoint,
            "http://127.0.0.1:9001"
        ));
    }

    #[test]
    fn dev_origin_ignores_hostless_urls() {
        assert!(!is_dev_origin_configured(
            &url("mailto:a@b.c"),
            "http://127.0.0.1:9000"
        ));
    }

    #[test]
    fn dev_ca_fails_closed_on_missing_or_garbage_file() {
        assert!(dev_tls_config_from_path("/nonexistent/cordy-dev-ca.pem").is_none());

        let mut garbage = std::env::temp_dir();
        garbage.push("cordy-remotemcp-garbage.pem");
        std::fs::write(&garbage, b"not a pem file").unwrap();
        let parsed = garbage.to_str().map(dev_tls_config_from_path);
        std::fs::remove_file(&garbage).ok();
        assert!(matches!(parsed, Some(None)), "garbage pem must fail closed");
    }

    #[test]
    fn dev_ca_loads_system_pem_bundle_when_available() {
        let Ok(pem) = std::fs::read("/etc/ssl/cert.pem") else {
            return;
        };
        let mut path = std::env::temp_dir();
        path.push("cordy-remotemcp-system-bundle.pem");
        std::fs::write(&path, &pem).unwrap();
        let config = path.to_str().and_then(dev_tls_config_from_path);
        std::fs::remove_file(&path).ok();
        assert!(config.is_some());
    }
}
