//! One-release compatibility helpers for the Patchbay brand boundary.

use std::ffi::OsString;

const LEGACY_ENV_PREFIX: &str = "CORDY_"; // legacy-brand-compat
const PATCHBAY_ENV_PREFIX: &str = "PATCHBAY_";

/// Mirrors legacy branded environment variables into their Patchbay names.
///
/// Explicit `PATCHBAY_*` values always win. This runs before configuration is
/// read, so existing self-hosted deployments can roll forward without exposing
/// legacy names throughout the configuration layer.
pub fn install_legacy_env_aliases() {
    let aliases = std::env::vars_os()
        .filter_map(|(key, value)| alias_for(&key).map(|alias| (alias, value)))
        .collect::<Vec<_>>();

    for (alias, value) in aliases {
        if std::env::var_os(&alias).is_none() {
            std::env::set_var(alias, value);
        }
    }
}

fn alias_for(key: &std::ffi::OsStr) -> Option<OsString> {
    let key = key.to_str()?;
    let suffix = key.strip_prefix(LEGACY_ENV_PREFIX)?;
    Some(format!("{PATCHBAY_ENV_PREFIX}{suffix}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_the_legacy_product_prefix() {
        assert_eq!(
            alias_for(std::ffi::OsStr::new("CORDY_SERVER_URL")), // legacy-brand-compat
            Some(OsString::from("PATCHBAY_SERVER_URL"))
        );
        assert_eq!(alias_for(std::ffi::OsStr::new("DATABASE_URL")), None);
    }
}
