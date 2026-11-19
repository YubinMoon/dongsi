mod broadcast;
mod commands;
mod error;
mod hub;
mod live;
mod paths;
mod sample;
mod store;

use std::sync::Arc;

use tauri::{Manager, WebviewWindowBuilder};

use paths::{AppEnv, AppPaths};

/// 메인 창을 만든다. `tauri.conf.json`의 `main` 창 설정을 쓰고, 프로필이 있으면
/// 제목에 이름을 붙이고 WebView2 데이터 폴더를 따로 쓴다(두 인스턴스가 같이 뜨게).
fn create_main_window(app: &tauri::AppHandle, paths: &AppPaths) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let mut builder = WebviewWindowBuilder::from_config(app, &config)?;
    if let Some(dir) = &paths.webview_dir {
        builder = builder
            .title(paths.window_title(&config.title))
            .data_directory(dir.clone());
    }
    builder.build()?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 시험용 환경 변수는 폴더를 만들기 전에 검사한다. 틀리면 실제 사용자 데이터를 건드리지 않고 끝낸다.
    let env = match AppEnv::from_env() {
        Ok(env) => env,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

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
        .setup(move |app| {
            let paths = AppPaths::new(
                &app.path().app_data_dir()?,
                &app.path().app_local_data_dir()?,
                env.profile.as_deref(),
            );
            let store = store::Store::open(&paths.data_dir)?;
            app.manage(commands::AppState::new(store));
            app.manage(hub::Hub::new(Arc::new(hub::TauriHubHost::new(
                app.handle().clone(),
            ))));
            app.manage(broadcast::Broadcaster::default());
            create_main_window(app.handle(), &paths)?;
            app.manage(paths);
            app.manage(env);
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
            live::disconnect_server,
            live::connected_servers,
            live::watch_broadcast,
            live::stop_watch,
            live::request_keyframe,
            live::scan_local,
            live::open_watch_window,
            broadcast::list_capture_sources,
            broadcast::available_encoders,
            broadcast::quality_presets,
            broadcast::start_broadcast,
            broadcast::dev_start_test_broadcast,
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
