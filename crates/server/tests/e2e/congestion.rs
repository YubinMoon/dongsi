//! 혼잡 대응(D-081): 송출 BWE 추정(TWCC). 서버는 REMB를 보내지 않는다(D-104).

use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use dongsi_core::priority::TimerResolution;

use crate::client::Client;
use crate::media::{Pump, info};
use crate::target::Target;

/// 추정값을 기다리는 시간.
const WAIT: Duration = Duration::from_secs(12);

/// 전용 서버(기본 설정)에서도 송출 쪽 BWE가 추정값을 얻는다: 서버(str0m)가 TWCC 피드백을 보낸다.
/// 외부 서버(예: 이전 서버 이미지)에서도 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publisher_estimates_bandwidth() {
    let t = Target::shared_with(1).await;
    let mut a = Client::connect(&t, "BWE 방송자").await;
    let p = a.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut v = Client::connect(&t, "BWE 시청자").await;
    let _w = v.watch_connected(a.id, None).await;
    let mut pump = Pump::new(0xB1E0);
    let deadline = Instant::now() + WAIT;
    loop {
        pump.run(&p, 25, |_| VideoCodec::H264, |_| {}).await;
        let s = p.stats();
        if s.estimate_kbps.is_some() || Instant::now() > deadline {
            break;
        }
    }
    let s = p.stats();
    eprintln!("[e2e] 송출 BWE: {s:?}");
    assert!(
        s.estimate_kbps.is_some(),
        "송출 쪽 BWE 추정값이 생기지 않았습니다(서버가 TWCC 피드백을 보내지 않음?): {s:?}"
    );
    assert_eq!(s.max_kbps, 7_000);
    assert!(s.target_kbps >= 300 && s.target_kbps <= s.max_kbps, "{s:?}");
}

/// 측정용(`--ignored`): 송출량이 적을 때(약 0.6Mbps) 추정값이 시간에 따라 어떻게 움직이는지 1초마다 찍는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bwe_trace() {
    let secs: u32 = std::env::var("DONGSI_BWE_TRACE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let _timer = std::env::var("DONGSI_BWE_TRACE_TIMER")
        .is_ok()
        .then(TimerResolution::request_1ms);
    let t = Target::shared_with(1).await;
    let mut a = Client::connect(&t, "추정 기록 방송자").await;
    let p = a.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut v = Client::connect(&t, "추정 기록 시청자").await;
    let _w = v.watch_connected(a.id, None).await;
    let mut pump = Pump::new(0xB1E2);
    let start = Instant::now();
    for _ in 0..secs {
        pump.run(&p, 50, |_| VideoCodec::H264, |_| {}).await;
        let s = p.stats();
        eprintln!(
            "[bwe] t={:>5.1}s video={:>6.0} est={:?} target={} limited={}",
            start.elapsed().as_secs_f32(),
            s.video_kbps,
            s.estimate_kbps,
            s.target_kbps,
            s.limited
        );
    }
}
