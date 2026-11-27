//! dongsi 데스크톱 앱.
//!
//! Tauri 명령(`commands`, `live`, `broadcast`)은 얇은 감싸개이고, 로직은 Tauri 없이 부를 수 있는
//! 모듈에 있다: 서버 연결(`hub`), 내장 서버(`embedded`), 방송의 네트워크 쪽(`session`),
//! 시청·감지 목록(`viewing`). 통합 시험(`tests/`)이 이 모듈을 직접 쓴다.

pub mod broadcast;
pub mod commands;
pub mod dev_probe;
pub mod discord;
pub mod embedded;
pub mod error;
pub mod follow;
pub mod hub;
pub mod live;
pub mod logfile;
pub mod paths;
mod sample;
pub mod session;
pub mod splash;
pub mod store;
pub mod viewing;

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

/// tracing을 시작한다. 개발 빌드는 stdout만. 배포 빌드는 콘솔이 없으므로 로그 파일에만 쓰고(D-088),
/// 파일은 폴더를 안 뒤에 `setup`에서 연다(그 전 줄은 메모리에 모은다). 돌려주는 싱크가 그 파일 쪽이다.
fn init_tracing() -> Option<logfile::LogSink> {
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new(
                "info,dongsi_desktop_lib=debug,dongsi_net=debug,dongsi_server=debug",
            )
        })
    };
    if cfg!(debug_assertions) {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter())
            .try_init();
        return None;
    }
    let sink = logfile::LogSink::default();
    tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_ansi(false)
        .with_writer(sink.clone())
        .try_init()
        .ok()?;
    // 배포 빌드는 stderr도 볼 수 없으므로 패닉도 로그 파일에 남긴다.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("패닉: {info}");
        default_hook(info);
    }));
    Some(sink)
}

/// 로그 파일을 열고 위치와 실행 환경을 한 줄씩 남긴다(`setup`에서 부른다).
fn attach_log_file(sink: &logfile::LogSink, local_base: &std::path::Path, profile: Option<&str>) {
    match sink.attach(&paths::log_dir(local_base, profile)) {
        Ok(path) => tracing::info!(
            "dongsi {} 시작, 로그 파일: {}",
            env!("CARGO_PKG_VERSION"),
            path.display()
        ),
        Err(e) => tracing::warn!("로그 파일을 열지 못했습니다: {e}"),
    }
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

    let log_sink = init_tracing();

    // 시작 창(작은 로딩 창). 메인 창은 숨겨서 만들고(tauri.conf.json visible=false), 프런트엔드가 첫 화면을
    // 준비하면(app_ready) 보이며 이 창을 닫는다. WebView2가 뜨는 동안 빈 창 대신 이 창이 보인다(splash.rs).
    splash::show();

    tauri::Builder::default()
        .setup(move |app| {
            let paths = AppPaths::new(
                &app.path().app_data_dir()?,
                &app.path().app_local_data_dir()?,
                env.profile.as_deref(),
            );
            if let Some(sink) = &log_sink {
                attach_log_file(
                    sink,
                    &app.path().app_local_data_dir()?,
                    env.profile.as_deref(),
                );
            }
            let store = store::Store::open(&paths.data_dir)?;
            app.manage(commands::AppState::new(store));
            app.manage(hub::Hub::new(Arc::new(hub::TauriHubHost::new(
                app.handle().clone(),
            ))));
            app.manage(broadcast::Broadcaster::default());
            app.manage(dev_probe::ToneProbeState::default());
            create_main_window(app.handle(), &paths)?;
            // 프런트엔드가 app_ready를 부르지 못해도(스크립트 오류 등) 메인 창은 보이게 한다.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(splash::FALLBACK);
                splash::show_main_window(&handle);
            });
            app.manage(paths);
            app.manage(env);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            splash::app_ready,
            commands::get_settings,
            commands::set_display_name,
            commands::list_servers,
            commands::parse_server_input,
            commands::add_server,
            commands::rename_server,
            commands::remove_server,
            commands::remember_password,
            commands::dev_sample_invite,
            commands::watch_log,
            dev_probe::dev_clock,
            dev_probe::dev_tone_probe_start,
            dev_probe::dev_tone_probe_take,
            dev_probe::dev_tone_probe_stop,
            live::connect_server,
            live::disconnect_server,
            live::connected_servers,
            live::watch_broadcast,
            live::stop_watch,
            live::request_keyframe,
            live::scan_local,
            live::broadcast_thumbnail,
            live::open_watch_window,
            broadcast::list_capture_sources,
            broadcast::capture_source_thumbnails,
            broadcast::video_encoder,
            broadcast::start_broadcast,
            broadcast::dev_start_test_broadcast,
            broadcast::broadcast_status,
            broadcast::broadcast_preview,
            broadcast::update_broadcast,
            broadcast::switch_broadcast_source,
            broadcast::set_broadcast_audio_boost,
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
