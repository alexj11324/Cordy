use std::env;

fn main() {
    emit("PATCHBAY_BUILD_VERSION", "dev");
    emit("PATCHBAY_BUILD_COMMIT", "unknown");
    emit("PATCHBAY_BUILD_DATE", "unknown");
    emit_value(
        "PATCHBAY_BUILD_OS",
        release_os(&env::var("CARGO_CFG_TARGET_OS").unwrap_or_else(|_| "unknown".into())),
    );
    emit_value(
        "PATCHBAY_BUILD_ARCH",
        release_arch(&env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "unknown".into())),
    );
}

fn emit(name: &str, fallback: &str) {
    println!("cargo:rerun-if-env-changed={name}");
    let value = env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.into());
    emit_value(name, &value);
}

fn emit_value(name: &str, value: &str) {
    println!("cargo:rustc-env={name}={value}");
}

fn release_os(target: &str) -> &str {
    match target {
        "macos" => "darwin",
        other => other,
    }
}

fn release_arch(target: &str) -> &str {
    match target {
        "x86" => "386",
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "powerpc" => "ppc",
        "powerpc64" => "ppc64",
        "riscv64" => "riscv64",
        "s390x" => "s390x",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_names_match_release_artifact_contract() {
        assert_eq!(release_os("macos"), "darwin");
        assert_eq!(release_arch("x86_64"), "amd64");
        assert_eq!(release_arch("aarch64"), "arm64");
        assert_eq!(release_os("linux"), "linux");
    }
}
