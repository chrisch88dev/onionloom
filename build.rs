//! Compile-time project metadata.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_GPU");
    println!("cargo:rerun-if-env-changed=ONIONLOOM_BUILD_TUNING");
    println!("cargo:rustc-env=ONIONLOOM_OWNER=chrisch88dev");
    println!("cargo:rustc-env=ONIONLOOM_WEBSITE=https://chrisch88.dev");
    println!("cargo:rustc-env=ONIONLOOM_CONTACT=chris@chrisch88.dev");
    println!(
        "cargo:rustc-env=ONIONLOOM_COPYRIGHT=Copyright (c) 2026 chrisch88dev and Onionloom contributors"
    );
    let cpu = if matches!(
        std::env::var("ONIONLOOM_BUILD_TUNING").as_deref(),
        Ok("host-native")
    ) {
        "host-native CPU"
    } else {
        "portable CPU"
    };
    let engines = if std::env::var_os("CARGO_FEATURE_GPU").is_some() {
        format!("{cpu} and automatic GPU")
    } else {
        cpu.to_owned()
    };
    println!("cargo:rustc-env=ONIONLOOM_ENGINES={engines}");
}
