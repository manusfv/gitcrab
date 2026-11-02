fn main() {
    println!("cargo:rerun-if-env-changed=GITCRAB_NATIVE_LIB_DIR");
    if let Ok(path) = std::env::var("GITCRAB_NATIVE_LIB_DIR") {
        println!("cargo:rustc-link-search=native={path}");
    }
    #[cfg(feature = "desktop")]
    tauri_build::build();
}
