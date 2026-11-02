#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use gitcrab_core::service::{Request, Response, Service};

#[cfg(all(feature = "native-smoke", not(debug_assertions)))]
compile_error!("The native-smoke automation server must never be included in release packages");

#[tauri::command]
async fn execute(request: Request, service: tauri::State<'_, Service>) -> Result<Response, String> {
    service
        .submit(request)?
        .await
        .map_err(|_| "Git worker stopped. Restart GitCrab.".into())
}
#[tauri::command]
async fn pick_repository() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Open Git repository")
            .pick_folder()
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())
}
#[tauri::command]
fn startup_repository() -> Option<String> {
    std::env::args().nth(1)
}

fn main() {
    let builder = tauri::Builder::default();
    #[cfg(feature = "native-smoke")]
    let builder = builder.plugin(tauri_plugin_wdio_webdriver::init());
    builder
        .manage(Service::new())
        .invoke_handler(tauri::generate_handler![
            execute,
            pick_repository,
            startup_repository
        ])
        .run(tauri::generate_context!())
        .expect("run GitCrab");
}
