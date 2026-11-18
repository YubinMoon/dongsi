mod broadcast;
mod commands;
mod error;
mod hub;
mod live;
mod sample;
mod store;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "info,dongsi_desktop_lib=debug,dongsi_net=debug,dongsi_server=debug",
                )
            }),
        )
        .try_init();

    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let store = store::Store::open(&dir)?;
            app.manage(commands::AppState::new(store));
            app.manage(hub::Hub::default());
            app.manage(broadcast::Broadcaster::default());
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
            commands::remember_password,
            commands::dev_sample_invite,
            commands::dev_log,
            live::connect_server,
            live::watch_broadcast,
            live::stop_watch,
            live::request_keyframe,
            live::scan_local,
            live::open_watch_window,
            broadcast::list_capture_sources,
            broadcast::available_encoders,
            broadcast::quality_presets,
            broadcast::start_broadcast,
            broadcast::broadcast_status,
            broadcast::update_broadcast,
            broadcast::stop_broadcast,
        ])
        .build(tauri::generate_context!())
        .expect("앱을 실행하지 못했습니다")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // 방송 중이면 서버에 알리고 UPnP 매핑을 지운다.
                broadcast::shutdown_on_exit(app);
            }
        });
}
