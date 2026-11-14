fn main() {
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
        // Build scripts run in the package directory, two levels below the checkout.
        Err(_) if std::path::Path::new("../../.git").exists() => "local build".into(),
        Err(_) => "source archive".into(),
    };
    let diagnostic = if cfg!(feature = "native-evidence") {
        " (diagnostics)"
    } else {
        ""
    };
    println!("cargo:rustc-env=GITCRAB_BUILD={revision}{diagnostic}");
}
