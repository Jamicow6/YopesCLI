fn main() {
    println!("cargo:rerun-if-env-changed=YOPES_BUILD_VERSION");
    if let Ok(version) = std::env::var("YOPES_BUILD_VERSION") {
        println!("cargo:rustc-env=YOPES_BUILD_VERSION={version}");
    }
}
