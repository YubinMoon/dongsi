//! 방송 중 캡처 대상 바꾸기(게임 창 따라가기, D-069) 시험. 이 PC의 실제 화면을 캡처한다(인코딩 결과는
//! 메모리에만 둔다). 창을 닫는 시험은 작은 창을 잠깐 띄우므로 `--ignored`로 따로 돌린다.
//! 전체 시험에만 든다(AGENTS.md "빌드·테스트"): `cargo test -p dongsi-media --features full-tests --test switch_source`

use std::sync::mpsc;
use std::time::{Duration, Instant};

use dongsi_core::VideoSettings;
use dongsi_media::{EncodedVideo, SourceId, SourceKind, VideoPipeline, list_sources};

struct Seen {
    keyframe: bool,
}

fn start(source: SourceId) -> (VideoPipeline, mpsc::Receiver<Seen>) {
    let (tx, rx) = mpsc::channel();
    let settings = VideoSettings {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_kbps: 2_500,
    };
    let p = VideoPipeline::start(
        source,
        settings,
        Box::new(move |f: EncodedVideo| {
            let _ = tx.send(Seen {
                keyframe: f.keyframe,
            });
        }),
    )
    .expect("비디오 파이프라인 시작");
    (p, rx)
}

/// `within` 안에 키프레임이 오면 true. 화면이 멈춰 있어도 키프레임 요청으로 마지막 화면이 다시 나온다.
fn keyframe_within(p: &VideoPipeline, rx: &mpsc::Receiver<Seen>, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    p.request_keyframe();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(s) if s.keyframe => return true,
            Ok(_) => {}
            Err(_) => p.request_keyframe(),
        }
    }
    false
}

fn monitors() -> Vec<SourceId> {
    list_sources()
        .unwrap()
        .into_iter()
        .filter(|s| s.kind == SourceKind::Monitor)
        .map(|s| s.id)
        .collect()
}

#[test]
fn switch_source_keeps_the_stream_going() {
    let mons = monitors();
    let (p, rx) = start(mons[0].clone());
    assert!(
        keyframe_within(&p, &rx, Duration::from_secs(5)),
        "처음 키프레임"
    );
    // 모니터가 둘 이상이면 두 번째로, 하나면 같은 모니터를 새로 연다.
    let next = mons.get(1).unwrap_or(&mons[0]).clone();
    p.switch_source(next).expect("다른 모니터로 바꾸기");
    while rx.try_recv().is_ok() {}
    assert!(
        keyframe_within(&p, &rx, Duration::from_secs(5)),
        "바꾼 뒤 키프레임"
    );
    // 열 수 없는 대상이면 오류이고, 지금 대상으로 계속 돈다.
    assert!(p.switch_source(SourceId::Window(1)).is_err());
    assert!(p.is_running());
    assert!(!p.is_waiting_source());
    while rx.try_recv().is_ok() {}
    assert!(
        keyframe_within(&p, &rx, Duration::from_secs(5)),
        "실패 뒤에도 계속"
    );
}

#[test]
fn test_pattern_cannot_switch() {
    let (p, _rx) = start(SourceId::TestPattern);
    let mons = monitors();
    assert!(p.switch_source(mons[0].clone()).is_err());
    assert!(p.is_running());
}

mod window {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
        TranslateMessage, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };
    use windows::core::w;

    /// 왼쪽 위에 작은 빈 창(시스템 STATIC 클래스)을 띄운다. 같은 스레드에서 [`close`]로 닫는다.
    pub fn open() -> isize {
        // SAFETY: 시스템 클래스로 창을 만들고, 같은 스레드에서 닫는다.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("dongsi switch test"),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
                320,
                180,
                None,
                None,
                None,
                None,
            )
        }
        .expect("시험 창");
        pump();
        hwnd.0 as isize
    }

    pub fn close(h: isize) {
        // SAFETY: open()이 같은 스레드에서 만든 창이다.
        unsafe { DestroyWindow(HWND(h as *mut _)) }.expect("창 닫기");
        pump();
    }

    pub fn pump() {
        let mut msg = MSG::default();
        // SAFETY: 이 스레드의 메시지만 처리한다.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    pub fn wait(d: std::time::Duration) {
        let end = std::time::Instant::now() + d;
        while std::time::Instant::now() < end {
            pump();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// 창이 닫혀도 기다리고(hold), 새 대상으로 바꾸면 이어지며, 기다리는 시간이 지나면 끝난다.
#[test]
#[ignore = "작은 창을 잠깐 띄운다: cargo test -p dongsi-media --features full-tests --test switch_source -- --ignored"]
fn hold_waits_for_a_new_source_after_the_window_closes() {
    let mons = monitors();
    let first = window::open();
    let (p, rx) = start(SourceId::Window(first));
    p.set_hold_on_close(Some(Duration::from_secs(10)));
    window::wait(Duration::from_millis(500));
    assert!(
        keyframe_within(&p, &rx, Duration::from_secs(5)),
        "창 캡처 키프레임"
    );

    window::close(first);
    window::wait(Duration::from_millis(1500));
    assert!(p.is_running(), "닫혀도 방송은 계속: {:?}", p.last_error());
    assert!(p.is_waiting_source());
    // 기다리는 동안 키프레임 요청에는 마지막 화면을 다시 낸다.
    while rx.try_recv().is_ok() {}
    assert!(
        keyframe_within(&p, &rx, Duration::from_secs(3)),
        "기다리는 중 키프레임"
    );

    p.switch_source(mons[0].clone()).expect("모니터로 바꾸기");
    assert!(!p.is_waiting_source());
    assert!(
        keyframe_within(&p, &rx, Duration::from_secs(5)),
        "바꾼 뒤 키프레임"
    );

    // 기다리는 시간이 지나면 끝난다.
    let second = window::open();
    p.switch_source(SourceId::Window(second))
        .expect("새 창으로 바꾸기");
    p.set_hold_on_close(Some(Duration::from_millis(800)));
    window::close(second);
    window::wait(Duration::from_millis(2500));
    assert!(!p.is_running());
    assert!(p.last_error().unwrap_or_default().contains("닫혔"));
}
