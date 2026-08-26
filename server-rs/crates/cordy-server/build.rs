use std::env;

fn main() {
    emit_version();
    for name in [
        "CORDY_BUILD_COMMIT",
        "CORDY_BUILD_DATE",
        "CORDY_BUILD_GO_VERSION",
        "CORDY_GIT_COMMIT",
    ] {
        emit(name, "unknown");
    }
}

fn emit_version() {
    println!("cargo:rerun-if-env-changed=CORDY_BUILD_VERSION");
    let version = env::var("CORDY_BUILD_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| env::var("CARGO_PKG_VERSION").ok())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CORDY_BUILD_VERSION={version}");
}

fn emit(name: &str, fallback: &str) {
    println!("cargo:rerun-if-env-changed={name}");
    let value = env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    println!("cargo:rustc-env={name}={value}");
}
