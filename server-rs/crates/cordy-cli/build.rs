use std::env;

fn main() {
    emit("CORDY_BUILD_VERSION", "dev");
    emit("CORDY_BUILD_COMMIT", "unknown");
    emit("CORDY_BUILD_DATE", "unknown");
    emit("CORDY_BUILD_GO_VERSION", "unknown");
    emit_value(
        "CORDY_BUILD_OS",
        go_os(&env::var("CARGO_CFG_TARGET_OS").unwrap_or_else(|_| "unknown".into())),
    );
    emit_value(
        "CORDY_BUILD_ARCH",
        go_arch(&env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "unknown".into())),
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

fn go_os(target: &str) -> &str {
    match target {
        "macos" => "darwin",
        other => other,
    }
}

fn go_arch(target: &str) -> &str {
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
    fn target_names_match_go_version_contract() {
        assert_eq!(go_os("macos"), "darwin");
        assert_eq!(go_arch("x86_64"), "amd64");
        assert_eq!(go_arch("aarch64"), "arm64");
        assert_eq!(go_os("linux"), "linux");
    }
}
