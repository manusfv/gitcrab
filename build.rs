fn main() {
    println!("cargo:rerun-if-changed=src");
    let branch = std::process::Command::new("git")
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    for item in std::iter::once("HEAD").chain(branch.as_deref()) {
        if let Ok(output) = std::process::Command::new("git")
            .args(["rev-parse", "--git-path", item])
            .output()
        {
            if output.status.success() {
                println!(
                    "cargo:rerun-if-changed={}",
                    String::from_utf8_lossy(&output.stdout).trim()
                );
            }
        }
    }
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "source archive".into());
    let dirty = std::process::Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .status()
        .is_ok_and(|s| !s.success());
    println!(
        "cargo:rustc-env=GITCRAB_BUILD={revision}{}",
        if dirty { " + changes" } else { "" }
    );
    println!("cargo:rerun-if-env-changed=GITCRAB_NATIVE_LIB_DIR");
    if let Ok(path) = std::env::var("GITCRAB_NATIVE_LIB_DIR") {
        println!("cargo:rustc-link-search=native={path}");
    }
}
