//! 게임 창 따라가기(D-069) 전체 흐름: 이 시험 실행 파일을 롤 이름(`LeagueClientUx.exe`, `League of Legends.exe`)으로
//! 복사해 작은 창을 띄우고, 실제 캡처·오디오 파이프라인과 [`Follower`]로 클라이언트 → 게임 → 클라이언트를 따라가는지 본다.
//! 창을 몇 초 띄우므로 `--ignored`로 따로 돌린다:
//! `cargo test -p dongsi-desktop --test follow_lol -- --ignored --test-threads 1`

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dongsi_core::VideoSettings;
use dongsi_desktop_lib::follow::{FollowKind, Follower, HOLD, follow_kind, lock_source};
use dongsi_media::{AudioPipeline, AudioSource, SourceInfo, VideoPipeline, list_sources};

const HELPER_ENV: &str = "DONGSI_FAKE_WINDOW";

/// 복사본이 실행하는 도우미: 제목이 `DONGSI_FAKE_WINDOW`인 작은 창을 띄우고 죽을 때까지 메시지를 돌린다.
#[test]
#[ignore = "follows_fake_lol_windows가 복사본에서 부른다"]
fn fake_window_helper() {
    let Ok(spec) = std::env::var(HELPER_ENV) else {
        return;
    };
    let (title, x) = spec.split_once('|').unwrap_or((spec.as_str(), "0"));
    window::run(title, x.parse().unwrap_or(0));
}

mod window {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, MSG, TranslateMessage, WS_CAPTION,
        WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_POPUP, WS_VISIBLE,
    };
    use windows::core::{HSTRING, w};

    pub fn run(title: &str, x: i32) {
        let title = HSTRING::from(title);
        // SAFETY: 시스템 STATIC 클래스로 창을 만들고 이 스레드에서 메시지를 돌린다.
        unsafe {
            let _hwnd = CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                &title,
                WS_POPUP | WS_CAPTION | WS_VISIBLE,
                x,
                0,
                320,
                200,
                None,
                None,
                None,
                None,
            )
            .expect("가짜 창");
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

struct Fake(Child);

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_fake(dir: &Path, exe: &str, title: &str, x: i32) -> Fake {
    let path = dir.join(exe);
    if !path.exists() {
        std::fs::copy(std::env::current_exe().unwrap(), &path).expect("시험 실행 파일 복사");
    }
    let child = Command::new(&path)
        .args(["--ignored", "--exact", "fake_window_helper", "--nocapture"])
        .env(HELPER_ENV, format!("{title}|{x}"))
        .spawn()
        .expect("가짜 창 실행");
    Fake(child)
}

fn find_window(pid: u32) -> Option<SourceInfo> {
    list_sources()
        .ok()?
        .into_iter()
        .find(|s| s.process_id == Some(pid))
}

fn wait_for<T>(what: &str, within: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + within;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < end, "{what}: 시간 안에 되지 않았습니다");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "작은 창 두 개를 몇 초 띄운다"]
fn follows_fake_lol_windows() {
    let dir: PathBuf = std::env::temp_dir().join(format!("dongsi-follow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let client = spawn_fake(&dir, "LeagueClientUx.exe", "League of Legends", 0);
    let client_win = wait_for("클라이언트 창", Duration::from_secs(10), || {
        find_window(client.0.id())
    });
    assert_eq!(follow_kind(&client_win), Some(FollowKind::Lol));

    let video = VideoPipeline::start(
        client_win.id.clone(),
        VideoSettings {
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_kbps: 2_500,
            ..VideoSettings::DEFAULT
        },
        Box::new(|_| {}),
    )
    .expect("비디오");
    video.set_hold_on_close(Some(HOLD));
    let audio = AudioPipeline::start(
        AudioSource::ProcessTree(client.0.id()),
        128,
        Box::new(|_| {}),
    )
    .expect("오디오");
    let current = Arc::new(Mutex::new(client_win.clone()));
    let mut follower = Follower::new(FollowKind::Lol, current.clone(), Some(audio.control()));
    let tick_until = |follower: &mut Follower, what: &str, ok: &dyn Fn() -> bool| {
        wait_for(what, Duration::from_secs(10), || {
            follower.tick(&video);
            ok().then_some(())
        })
    };

    // 게임이 시작되면 게임 창으로, 소리도 게임 프로세스로.
    let game = spawn_fake(
        &dir,
        "League of Legends.exe",
        "League of Legends (TM) Client",
        340,
    );
    let game_pid = game.0.id();
    let game_win = wait_for("게임 창", Duration::from_secs(10), || {
        find_window(game_pid)
    });
    tick_until(&mut follower, "게임 창으로 바꾸기", &|| {
        lock_source(&current).id == game_win.id
    });
    assert!(video.is_running());
    wait_for("게임 소리", Duration::from_secs(5), || {
        audio
            .stats()
            .mode
            .contains(&format!("pid {game_pid}"))
            .then_some(())
    });

    // 게임이 끝나면(창이 닫히면) 방송은 이어지고 클라이언트로 돌아온다.
    drop(game);
    tick_until(&mut follower, "클라이언트로 돌아오기", &|| {
        lock_source(&current).id == client_win.id
    });
    assert!(video.is_running(), "{:?}", video.last_error());
    assert!(!video.is_waiting_source());
    let client_pid = client.0.id();
    wait_for("클라이언트 소리", Duration::from_secs(5), || {
        audio
            .stats()
            .mode
            .contains(&format!("pid {client_pid}"))
            .then_some(())
    });

    drop(client);
    drop(follower);
    audio.stop();
    video.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
