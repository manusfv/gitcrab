//! Native notifications, constructed off the UI thread. No polling or Git writes.
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
pub type Change = (Option<String>, std::time::Instant);

pub fn start(
    root: String,
) -> Result<(RecommendedWatcher, async_channel::Receiver<Change>), String> {
    let root = PathBuf::from(root);
    let resolve = |arg: &str| -> Result<PathBuf, String> {
        let mut command = std::process::Command::new(gitcrab_core::platform::git_executable());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let output = command
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--path-format=absolute", arg])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        ))
    };
    let git = resolve("--git-dir")?;
    let common = resolve("--git-common-dir")?;
    let metadata = [git.clone(), common.clone()];
    let (tx, rx) = async_channel::bounded(1);
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        match result {
            Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                let relevant = event.paths.iter().any(|p| {
                    // Git objects/logs/cache locks do not change the visible workspace.
                    if p.extension().is_some_and(|e| e == "lock") {
                        return false;
                    }
                    for base in &metadata {
                        if let Ok(relative) = p.strip_prefix(base) {
                            return relative == Path::new("HEAD")
                                || relative == Path::new("index")
                                || relative == Path::new("packed-refs")
                                || relative == Path::new("config")
                                || relative.starts_with("refs");
                        }
                    }
                    true
                });
                if relevant {
                    let _ = tx.try_send((None, std::time::Instant::now()));
                }
            }
            Err(e) => {
                let _ = tx.try_send((Some(e.to_string()), std::time::Instant::now()));
            }
            _ => {}
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    for directory in [git, common] {
        if !directory.starts_with(&root) {
            watcher
                .watch(&directory, RecursiveMode::Recursive)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok((watcher, rx))
}
