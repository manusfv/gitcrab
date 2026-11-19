fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=gitcrab.rc");
        println!("cargo:rerun-if-changed=../../icons/gitcrab.ico");
        println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
        let env = |k: &str| std::env::var(k).expect("cargo sets package version vars");
        let macros = [
            format!("VER_MAJOR={}", env("CARGO_PKG_VERSION_MAJOR")),
            format!("VER_MINOR={}", env("CARGO_PKG_VERSION_MINOR")),
            format!("VER_PATCH={}", env("CARGO_PKG_VERSION_PATCH")),
            format!("VER_STRING=\"{}\"", env("CARGO_PKG_VERSION")),
        ];
        embed_resource::compile_for("gitcrab.rc", ["gitcrab"], macros)
            .manifest_required()
            .expect("embed GitCrab's Windows icon and version info");
    }

    println!("cargo:rerun-if-env-changed=GITCRAB_NATIVE_LIB_DIR");
    if let Ok(path) = std::env::var("GITCRAB_NATIVE_LIB_DIR") {
        println!("cargo:rustc-link-search=native={path}");
    }
}
