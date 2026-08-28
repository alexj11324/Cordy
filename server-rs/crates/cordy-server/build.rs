fn main() {
    println!("cargo:rerun-if-env-changed=CORDY_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=CORDY_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=CORDY_GIT_COMMIT");

    let version = std::env::var("CORDY_BUILD_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "dev".into()));
    let commit = std::env::var("CORDY_BUILD_COMMIT")
        .or_else(|_| std::env::var("CORDY_GIT_COMMIT"))
        .unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=CORDY_EFFECTIVE_BUILD_VERSION={version}");
    println!("cargo:rustc-env=CORDY_EFFECTIVE_BUILD_COMMIT={commit}");
}
