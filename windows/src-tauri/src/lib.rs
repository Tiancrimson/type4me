use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    app_name: &'static str,
    version: &'static str,
    platform: &'static str,
    architecture: &'static str,
    development_build: bool,
    backend_connected: bool,
}

#[tauri::command]
fn get_runtime_status() -> RuntimeStatus {
    RuntimeStatus {
        app_name: "Type4Me",
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        development_build: cfg!(debug_assertions),
        backend_connected: true,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_runtime_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
