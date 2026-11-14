fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=gitcrab.rc");
        println!("cargo:rerun-if-changed=../../icons/gitcrab.ico");
        embed_resource::compile_for("gitcrab.rc", ["gitcrab"], embed_resource::NONE)
            .manifest_required()
            .expect("embed GitCrab's Windows icon");
    }

    println!("cargo:rerun-if-env-changed=GITCRAB_NATIVE_LIB_DIR");
    if let Ok(path) = std::env::var("GITCRAB_NATIVE_LIB_DIR") {
        println!("cargo:rustc-link-search=native={path}");
    }
}
