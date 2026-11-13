fn main() {
    #[cfg(all(windows, feature = "gpui-desktop"))]
    {
        println!("cargo:rerun-if-changed=icons/gitcrab.rc");
        println!("cargo:rerun-if-changed=icons/gitcrab.ico");
        embed_resource::compile_for("icons/gitcrab.rc", ["gitcrab"], embed_resource::NONE)
            .manifest_required()
            .expect("embed GitCrab's Windows icon");
    }

    println!("cargo:rerun-if-env-changed=GITCRAB_BUILD_REVISION");
    let revision = match std::env::var("GITCRAB_BUILD_REVISION") {
        Ok(revision) => {
            assert!(
                matches!(revision.len(), 40 | 64)
                    && revision.bytes().all(|b| b.is_ascii_hexdigit()),
                "GITCRAB_BUILD_REVISION must be a full Git object ID"
            );
            revision
        }
        Err(_) if std::path::Path::new(".git").exists() => "local build".into(),
        Err(_) => "source archive".into(),
    };
    let diagnostic = if cfg!(feature = "native-evidence") {
        " (diagnostics)"
    } else {
        ""
    };
    println!("cargo:rustc-env=GITCRAB_BUILD={revision}{diagnostic}");
    println!("cargo:rerun-if-env-changed=GITCRAB_NATIVE_LIB_DIR");
    if let Ok(path) = std::env::var("GITCRAB_NATIVE_LIB_DIR") {
        println!("cargo:rustc-link-search=native={path}");
    }
}
