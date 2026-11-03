//! Opt-in validation measurements. Excluded from production builds.
use gpui::*;
use std::time::Instant;
pub fn record(kind: &'static str, started: Instant, cx: &App) {
    let Ok(directory) = std::env::var("GITCRAB_INTERACTION_TRACE") else {
        return;
    };
    let elapsed = started.elapsed().as_secs_f64() * 1000.;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let data = serde_json::json!({"kind":kind,"elapsed_ms":elapsed,"build":env!("GITCRAB_BUILD"),"physical_presentation":false});
    cx.background_executor()
        .spawn(async move {
            let directory = std::path::PathBuf::from(directory);
            if std::fs::create_dir_all(&directory).is_ok() {
                let _ = std::fs::write(
                    directory.join(format!("{kind}-{stamp}.json")),
                    serde_json::to_vec_pretty(&data).unwrap(),
                );
            }
        })
        .detach();
}
