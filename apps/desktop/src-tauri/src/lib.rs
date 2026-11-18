mod commands;
mod error;
mod sample;
mod store;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let store = store::Store::open(&dir)?;
            app.manage(commands::AppState::new(store));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_display_name,
            commands::list_servers,
            commands::parse_server_input,
            commands::add_server,
            commands::rename_server,
            commands::remove_server,
            commands::list_broadcasts,
            commands::list_local_broadcasts,
            commands::remember_password,
            commands::dev_sample_invite,
        ])
        .run(tauri::generate_context!())
        .expect("앱을 실행하지 못했습니다");
}
