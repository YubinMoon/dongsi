//! 손실 복구 시험(D-087): 실제 서버 경로(방송자 str0m → SFU → 시청자 str0m)에서 손실을 결정적으로 넣어
//! SFU의 복구 정책을 확인한다(`sfu.rs` 머리 문서 "끊긴 뒤 키프레임까지 비디오 전달 중단", "키프레임 요청",
//! "시청자 송신 스트림의 재전송(RTX)"). 손실은 시험 클라이언트 소켓 앞에서 `impair.rs`로 넣는다.
//!
//! 방송자는 `RawPublisher`의 인코더 흉내(델타 프레임 + PLI에 키프레임)다. 모든 시험이 외부 서버에서도 돈다(서버 안쪽
//! 상태를 읽지 않고 방송자·시청자 쪽 관찰로만 단언한다. 서버 지표 단언은 지표가 들어오면 `Target::metrics`가 있을 때만
//! 더한다). 시험마다 방송 1개·시청자 2명 이하를 쓴다.
//!
//! # (a) 복구 불가능한 손실 뒤 깨진 비디오는 가지 않고 키프레임에서 이어진다
//!
//! 방송자가 비디오 원본과 RTX를 400ms 버리고, RTX는 1.9초까지 계속 버린다(SFU의 NACK는 5번까지 하고 포기한다:
//! str0m `streams/register_nack.rs` `MAX_NACKS = 5`, 재시도는 RTT + 5ms마다, 포기는 마지막 NACK + RTT × 3 뒤.
//! 첫 RTT를 모르면 100ms로 쳐서 최대 약 0.8초 안에 끝난다. 1.5초는 그 두 배쯤의 여유). 프레임이 만료되면 SFU가
//! `contiguous=false`를 보고 시청자에게 비디오를 보내지 않으며 방송자에게 PLI를 보낸다. 시청자가 받은 비디오의 인덱스(tick)에서:
//! - 빈틈이 하나 생기고, 빈틈 바로 뒤 비디오가 키프레임이다(=그 사이 보낸 델타 프레임이 하나도 안 갔다).
//!   빈틈은 손실 구간(400ms = 비디오 10개)만이 아니라 만료까지 방송자가 보낸 델타 프레임도 포함한다(`withheld`).
//! - 그동안 오디오는 계속 왔다(오디오 빈틈 없음).
//! - 방송자가 그 사이 PLI를 받았고, 그 뒤 키프레임을 보냈다. 키프레임 뒤로는 델타 프레임이 빈틈 없이 온다.
//!
//! # (b) 서버→시청자 몰린 손실에서 재전송이 이어진다(상한 0.5)
//!
//! 방송자는 12000바이트 프레임을 25fps로 보낸다(프레임 하나가 RTP 패킷 11개쯤, 비디오 약 2.4Mbps, 초당 패킷 약 275개).
//! 시청자가 받는 비디오 원본을 1.5초 동안 10개 중 3개씩(연속 3개, 손실률 30%) 버린다. RTX는 지나간다.
//!
//! 재전송 비율(str0m `streams/send.rs:863-872`)은 최근 1초의 `재전송 바이트 / (재전송 + 원본 바이트)`이고 상한을 넘으면
//! 대기 중인 재전송을 모두 지운다. 손실률이 p이고 손실을 모두 한 번씩 재전송하면 재전송 바이트는 p × 원본이므로
//! 비율은 p / (1 + p)다. p = 0.3이면 0.3 / 1.3 = 0.23이다.
//! - 옛 상한 0.15(`DEFAULT_RTX_RATIO_CAP`)는 p / (1 + p) > 0.15, 곧 p > 0.176부터 넘는다. 0.23 > 0.15이므로 손실이
//!   이어지는 동안 비율이 0.15를 넘는 순간부터 재전송이 지워진다. 지워진 재전송은 시청자의 NACK 재시도(최대 5번, 매
//!   RTT + 5ms)로만 다시 요청되고, 재전송이 지워지는 동안 비율이 내려와야 다시 나간다. 비율이 0.15 아래로 유지되려면
//!   손실의 0.15 / 0.85 / 0.3 = 59%만 복구할 수 있다. 나머지 41%는 NACK 5번을 다 쓰고 포기되어, 프레임 하나가 11패킷에
//!   30% 손실이면 거의 모든 프레임이 영향을 받으므로 시청자 쪽 인덱스에 빠진 프레임이 생긴다(시험은 이를 단언한다).
//! - 새 상한 0.5는 p / (1 + p) > 0.5, 곧 p > 1(손실이 원본보다 많음)일 때만 넘는다. 0.23 < 0.5라 재전송이 지워지지
//!   않고 모든 프레임이 온다.
//!
//! 시험은 시청자가 받은 모든 비디오·RTX 패킷의 기록(`Impair::record`)에서 최근 1초 재전송 비율의 최댓값을 다시 계산해
//! 0.15보다 크고 0.5보다 작음을 단언한다(옛 상한이면 지워졌을 상황이었음을 같이 확인한다). 코드를 0.15로 되돌려 돌려 보지는
//! 않았다.
//!
//! # (c) PLI 재시도 간격과 시청자별 제한
//!
//! - (c1) 방송자가 PLI에 답하지 않으면 SFU는 0.5초마다 PLI를 되풀이한다(`KEYFRAME_RETRY`). 방송자가 받은 PLI 간격이
//!   350~900ms이고 1초를 넘지 않는다(0.5초 + SFU 루프 주기 최대 100ms + 전송·스레드 지연). 그동안 시청자에게 비디오는 가지
//!   않고 오디오만 온다. 답하기를 켜면 키프레임이 오고, 그 뒤로 PLI가 멈춘다.
//! - (c2) 시청자 하나가 200ms마다 RTCP PLI와 시그널링 `RequestKeyframe`을 같이 보내도(같은 시청자 세션의 한도 하나를
//!   공유한다) 방송자가 받는 키프레임 요청은 2초에 한 번 안팎이다(4.5초 동안 2~3번, 간격 1.8초 이상). 그 한도가 걸려 있는
//!   동안 새 시청자가 들어오면 바로 PLI가 간다.

use std::time::{Duration, Instant};

use dongsi_core::InstallId;
use dongsi_core::VideoCodec;
use dongsi_core::proto::{ClientMsg, ServerMsg};
use str0m::media::{KeyframeRequestKind, MediaKind};

use crate::client::{Client, Watching, find};
use crate::impair::{Class, Classes, LogEntry};
use crate::media::info;
use crate::raw_publisher::{Encoder, PendingRawPublisher, RawOptions, RawPublisher};
use crate::target::Target;
use crate::viewer::{PendingViewer, Received, ViewerOptions};

/// 시험용 방송자 표시.
const TAG: u32 = 41;
/// (a)(c1) 방송자가 비디오 원본·RTX를 버리는 시간(비디오 프레임 10개).
const LOSS: Duration = Duration::from_millis(400);
/// (a)(c1) 그 뒤로 RTX만 더 버리는 시간(SFU가 NACK를 포기하고 만료할 때까지, 모듈 설명).
const RTX_EXTRA: Duration = Duration::from_millis(1500);

/// 방송을 시작한다(인코더 흉내). 프레임 페이로드는 `frame_bytes` 바이트쯤.
async fn publish(c: &mut Client, frame_bytes: usize) -> RawPublisher {
    let pending = PendingRawPublisher::new(c.server_ip());
    let req = c.next_req();
    c.send(ClientMsg::Publish {
        req,
        offer: pending.offer.clone(),
        info: info(VideoCodec::H264),
        password: None,
    });
    let answer = c
        .wait_for("Published", |m| match m {
            ServerMsg::Published { req: r, answer } if *r == req => Some(answer.clone()),
            _ => None,
        })
        .await;
    let p = pending.start_with(
        &answer,
        TAG,
        RawOptions {
            encoder: Some(Encoder { frame_bytes }),
        },
    );
    p.wait_connected(Duration::from_secs(10)).await;
    p
}

/// 방송을 보고 미디어 연결이 될 때까지 기다린다. 시청 쪽 수신 통계(NACK·PLI 수)를 켠다.
async fn watch(c: &mut Client, broadcaster: InstallId) -> Watching {
    let pending = PendingViewer::with_options(
        c.server_ip(),
        ViewerOptions {
            log_sr: false,
            stats: true,
        },
    );
    let req = c.next_req();
    c.send(ClientMsg::Watch {
        req,
        broadcaster,
        password: None,
        offer: pending.offer.clone(),
    });
    let (session, answer) = c
        .wait_for("Watching", |m| match m {
            ServerMsg::Watching {
                req: r,
                session,
                answer,
            } if *r == req => Some((*session, answer.clone())),
            _ => None,
        })
        .await;
    let w = Watching {
        session,
        viewer: pending.start(&answer),
    };
    w.viewer.wait_connected(Duration::from_secs(10)).await;
    w
}

/// 시청자가 받은 것을 `acc`에 모으며 `done`이 참이 되거나 `within`이 지날 때까지 기다린다.
async fn collect_until(
    w: &Watching,
    acc: &mut Vec<Received>,
    within: Duration,
    mut done: impl FnMut(&[Received]) -> bool,
) -> bool {
    let end = Instant::now() + within;
    loop {
        acc.extend(w.viewer.drain());
        if done(acc) {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 받은 비디오의 (인덱스, 키프레임인지)를 받은 순서대로.
fn video_seq(rs: &[Received]) -> Vec<(u32, bool)> {
    rs.iter()
        .filter(|r| r.kind == MediaKind::Video)
        .filter_map(|r| Some((r.index?, r.keyframe)))
        .collect()
}

/// 비디오 인덱스(짝수 tick, 방송자가 40ms마다 하나)에 빈틈이 있는 곳: (빈틈 앞 마지막 인덱스, 뒤 첫 인덱스, 뒤 첫 프레임이 키프레임인지).
fn holes(seq: &[(u32, bool)]) -> Vec<(u32, u32, bool)> {
    seq.windows(2)
        .filter(|w| w[1].0 > w[0].0 + 2)
        .map(|w| (w[0].0, w[1].0, w[1].1))
        .collect()
}

/// 방송이 흐르는 상태까지 기다린다: 키프레임 하나와 그 뒤 델타 프레임 `deltas`개.
async fn wait_streaming(w: &Watching, rs: &mut Vec<Received>, deltas: usize) {
    let ok = collect_until(w, rs, Duration::from_secs(10), |rs| {
        let seq = video_seq(rs);
        seq.iter().any(|s| s.1) && seq.iter().filter(|s| !s.1).count() >= deltas
    })
    .await;
    assert!(
        ok,
        "시청자가 10초 안에 키프레임과 델타 프레임 {deltas}개를 받지 못했습니다"
    );
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

/// 내부 서버의 지표 값들(`series` 순서). 외부 서버면 `None`(서버 안쪽 단언은 건너뛴다). 지표는 서버 합계라서 시험은
/// 시작과 끝의 차이(`delta`)로 본다(연결 때 보낸 PLI 같은 앞부분을 빼려고).
fn metrics_of(t: &Target, series: &[&str]) -> Option<Vec<u64>> {
    let text = t.metrics()?.render();
    Some(
        series
            .iter()
            .map(|s| {
                text.lines()
                    .filter(|l| !l.starts_with('#'))
                    .find_map(|l| l.strip_prefix(s)?.strip_prefix(' ')?.parse().ok())
                    .unwrap_or(0)
            })
            .collect(),
    )
}

fn delta(before: &Option<Vec<u64>>, after: &Option<Vec<u64>>) -> Option<Vec<u64>> {
    Some(
        after
            .as_ref()?
            .iter()
            .zip(before.as_ref()?)
            .map(|(a, b)| a - b)
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// (a) 복구 불가능한 손실 뒤 깨진 비디오는 가지 않고 키프레임에서 이어진다

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broken_video_withheld_until_keyframe() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "손실 방송자").await;
    let id = b.id;
    let p = publish(&mut b, 3000).await;
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = watch(&mut v, id).await;
    let mut rs = Vec::new();
    wait_streaming(&w, &mut rs, 10).await;
    assert!(
        holes(&video_seq(&rs)).is_empty(),
        "손실을 넣기 전에 이미 비디오 빈틈이 있습니다: {:?}",
        holes(&video_seq(&rs))
    );

    // 손실: 비디오 원본·RTX 400ms, RTX는 1.9초까지.
    const SERIES_A: [&str; 5] = [
        "dongsi_sfu_broken_video_frames_total",
        "dongsi_sfu_dropped_video_frames_total",
        "dongsi_sfu_pli_sent_total",
        "dongsi_sfu_keyframes_received_total",
        "dongsi_sfu_publisher_nacks_sent_total",
    ];
    let m0 = metrics_of(&t, &SERIES_A);
    let loss_tick = p.tick();
    let started = Instant::now();
    p.impair().drop_for(Classes::VIDEO_AND_RTX, LOSS);
    p.impair().drop_for(Classes::RTX, LOSS + RTX_EXTRA);
    let end_tick = loss_tick + (LOSS.as_millis() / 20) as u32;

    // 빈틈이 생기고 키프레임 뒤로 델타 프레임이 15개 이어질 때까지.
    let ok = collect_until(&w, &mut rs, Duration::from_secs(10), |rs| {
        let seq = video_seq(rs);
        holes(&seq)
            .first()
            .is_some_and(|h| seq.iter().filter(|s| s.0 > h.1 && !s.1).count() >= 15)
    })
    .await;
    let seq = video_seq(&rs);
    let hs = holes(&seq);
    let counters = p.impair().counters();
    eprintln!(
        "[e2e] (a) 방송자가 버린 패킷: 비디오 {} RTX {} / 빈틈 {hs:?} (손실 시작 tick {loss_tick}, 끝 tick {end_tick})",
        counters.video.dropped, counters.rtx.dropped
    );
    assert!(
        ok,
        "10초 안에 빈틈과 키프레임 뒤 델타 프레임을 받지 못했습니다: 빈틈 {hs:?}"
    );
    assert_eq!(hs.len(), 1, "비디오 빈틈이 하나여야 합니다: {hs:?}");
    let (before, after, after_is_key) = hs[0];

    // 빈틈 바로 뒤 비디오가 키프레임이다: 빈틈과 키프레임 사이에 델타 프레임이 하나도 가지 않았다.
    assert!(
        after_is_key,
        "빈틈({before}→{after}) 바로 뒤 비디오가 키프레임이 아닙니다(깨진 델타 프레임이 갔습니다)"
    );
    // 빈틈은 손실 구간 전체를 덮고(비디오 10개, 경계 2개 여유), 만료까지 방송자가 보낸 델타 프레임도 덮는다.
    let hole_frames = (after - before) / 2 - 1;
    let withheld = after.saturating_sub(end_tick) / 2;
    eprintln!(
        "[e2e] (a) 빈틈 {hole_frames}프레임({}ms), 손실 구간 뒤 보냈지만 가지 않은 프레임 {withheld}개, 키프레임 tick {after}",
        (after - before) * 20
    );
    assert!(
        hole_frames >= 8,
        "빈틈이 손실 구간보다 짧습니다: {hole_frames}프레임"
    );
    assert!(
        before < loss_tick + 3,
        "빈틈이 손실 시작({loss_tick}) 전에 시작합니다: {before}"
    );
    assert!(
        withheld >= 3,
        "손실 구간이 끝난 뒤 보낸 델타 프레임이 {withheld}개만 빠졌습니다(깨진 델타 프레임이 가지 않아야 함, 키프레임 tick {after}, 손실 끝 tick {end_tick})"
    );

    // 오디오는 계속 왔다: 빈틈 동안 인덱스가 빠지거나 크게 벌어지지 않았다.
    let audio: Vec<u32> = rs
        .iter()
        .filter(|r| r.kind == MediaKind::Audio)
        .filter_map(|r| r.index)
        .filter(|i| *i >= before && *i <= after)
        .collect();
    let span = (after - before + 1) as usize;
    let max_gap = audio.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0);
    eprintln!(
        "[e2e] (a) 빈틈 동안 오디오 {}/{span}개, 최대 간격 {max_gap}tick",
        audio.len()
    );
    // 오디오는 NACK가 없어 시험 PC에 부하가 있으면(전체 시험이 병렬로 돌 때) 루프백에서 몇 개 빠질 수 있다(측정: 혼자 돌 때
    // 37/37, 전체와 같이 돌 때 33/37, 최대 간격 3tick). 기존 시험(`MIN_RATIO`)처럼 80%와 최대 간격 10tick(200ms)을
    // 허용한다. 비디오 전달 중단이 오디오까지 막았다면 이 구간(0.7초)에 오디오가 0개다.
    assert!(
        audio.len() as f64 >= span as f64 * crate::MIN_RATIO,
        "빈틈 동안 오디오가 {}/{span}개뿐입니다",
        audio.len()
    );
    assert!(
        max_gap <= 10,
        "빈틈 동안 오디오가 {max_gap}tick(20ms 단위) 끊겼습니다"
    );

    // 방송자가 그 사이 PLI를 받았고, 그 뒤 그 키프레임을 보냈다.
    let reqs: Vec<_> = p
        .keyframe_requests()
        .into_iter()
        .filter(|r| r.at >= started)
        .collect();
    let key = p
        .keyframes_sent()
        .into_iter()
        .find(|k| k.tick == after)
        .unwrap_or_else(|| panic!("방송자가 보낸 키프레임 중 tick {after}가 없습니다"));
    eprintln!(
        "[e2e] (a) 손실 시작 뒤 PLI {}번(첫 번째 {}ms 뒤), 키프레임은 {}ms 뒤",
        reqs.len(),
        reqs.first().map_or(0, |r| ms(r.at - started)),
        ms(key.at - started)
    );
    assert!(
        reqs.first().is_some_and(|r| r.at <= key.at),
        "방송자가 키프레임을 보내기 전에 PLI를 받지 못했습니다"
    );
    assert!(
        counters.video.dropped >= 15,
        "손실이 적용되지 않았습니다(버린 비디오 패킷 {})",
        counters.video.dropped
    );
    assert!(
        counters.rtx.dropped >= 1,
        "SFU의 NACK에 대한 재전송이 버려지지 않았습니다(NACK가 오지 않았거나 RTX가 아님)"
    );

    // 서버 지표(내부 서버). NACK 수는 str0m 통계 이벤트(SFU는 1초 주기, `STATS_INTERVAL`)로 세므로 한 주기를 기다린다.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // 끊긴 프레임을 보고, 키프레임까지 비디오를 버리고, NACK·PLI를 보냈고, 키프레임을 받았다.
    if let Some(d) = delta(&m0, &metrics_of(&t, &SERIES_A)) {
        eprintln!(
            "[e2e] (a) 서버 지표 변화: 끊긴 프레임 {} / 버린 비디오 프레임 {} / PLI {} / 받은 키프레임 {} / 방송자에게 보낸 NACK {}",
            d[0], d[1], d[2], d[3], d[4]
        );
        assert!(
            d[0] >= 1,
            "서버가 끊긴 비디오 프레임(contiguous=false)을 세지 않았습니다"
        );
        assert!(
            d[1] >= u64::from(withheld).min(3),
            "서버가 버린 비디오 프레임이 {}개뿐입니다(가지 않은 프레임 {withheld}개)",
            d[1]
        );
        assert!(d[2] >= 1, "서버가 PLI를 보내지 않았습니다");
        assert!(d[3] >= 1, "서버가 키프레임을 받지 못했습니다");
        assert!(d[4] >= 1, "서버가 방송자에게 NACK를 보내지 않았습니다");
    }

    // 키프레임 뒤로는 델타 프레임이 다시 오고 빈틈이 없다.
    let tail: Vec<(u32, bool)> = seq.iter().copied().filter(|s| s.0 >= after).collect();
    assert!(
        tail.iter().filter(|s| !s.1).count() >= 15,
        "키프레임 뒤 델타 프레임이 {}개뿐입니다",
        tail.iter().filter(|s| !s.1).count()
    );
    assert!(tail[0].1, "키프레임으로 이어지지 않습니다");

    b.send(ClientMsg::Unpublish);
    drop(w);
    drop(p);
}

// ---------------------------------------------------------------------------
// (b) 서버→시청자 몰린 손실에서 재전송이 이어진다(상한 0.5)

/// 최근 `window` 동안의 `재전송 바이트 / (재전송 + 원본 바이트)`의 최댓값(str0m `streams/send.rs`의 재전송 비율과 같은 식).
fn max_rtx_ratio(log: &[LogEntry], window: Duration) -> f64 {
    let mut max = 0f64;
    let mut start = 0;
    let (mut orig, mut rtx) = (0u64, 0u64);
    for (i, e) in log.iter().enumerate() {
        match e.class {
            Class::Video => orig += e.len as u64,
            Class::Rtx => rtx += e.len as u64,
            _ => {}
        }
        while log[start].at + window < e.at {
            match log[start].class {
                Class::Video => orig -= log[start].len as u64,
                Class::Rtx => rtx -= log[start].len as u64,
                _ => {}
            }
            start += 1;
        }
        if i >= start && orig + rtx > 0 {
            max = max.max(rtx as f64 / (rtx + orig) as f64);
        }
    }
    max
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_loss_to_viewer_recovered_by_rtx() {
    const LOSS: Duration = Duration::from_millis(1500);
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "큰 프레임 방송자").await;
    let id = b.id;
    let p = publish(&mut b, 12_000).await;
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = watch(&mut v, id).await;
    w.viewer.impair().record();
    let mut rs = Vec::new();
    wait_streaming(&w, &mut rs, 25).await;

    // 서버에서 오는 비디오 원본을 1.5초 동안 10개 중 앞 3개씩 버린다(RTX는 지나간다).
    const SERIES_B: [&str; 2] = [
        "dongsi_sfu_viewer_nacks_received_total",
        "dongsi_sfu_rtx_packets_sent_total",
    ];
    let m0 = metrics_of(&t, &SERIES_B);
    let nacks_before = w.viewer.ingress().nacks;
    let loss_at = Instant::now();
    w.viewer
        .impair()
        .drop_pattern_for(Classes::VIDEO, 3, 10, LOSS);
    tokio::time::sleep(LOSS + Duration::from_millis(1000)).await;
    rs.extend(w.viewer.drain());
    tokio::time::sleep(Duration::from_millis(300)).await; // 마지막 통계(0.25초 주기)
    let c = w.viewer.impair().counters();
    let ingress = w.viewer.ingress();
    let log = w.viewer.impair().log();
    let ratio = max_rtx_ratio(&log, Duration::from_secs(1));
    let seq = video_seq(&rs);
    let hs = holes(&seq);
    let discontiguous = rs
        .iter()
        .filter(|r| r.kind == MediaKind::Video && !r.contiguous)
        .count();
    // 손실 구간(1.5초)에 서버가 보낸 비디오 원본 패킷 수와 손실률.
    let in_loss = log
        .iter()
        .filter(|e| e.class == Class::Video && e.at >= loss_at && e.at < loss_at + LOSS)
        .count();
    let loss_pct = 100.0 * c.video.dropped as f64 / in_loss.max(1) as f64;
    eprintln!(
        "[e2e] (b) 손실 구간의 비디오 원본 {in_loss}개 중 {}개 버림(손실률 {loss_pct:.0}%), RTX {}개 받음, 시청자 NACK {}번, 최근 1초 재전송 비율 최댓값 {ratio:.3}, 받은 프레임 {} 빈틈 {hs:?}, contiguous=false {discontiguous}",
        c.video.dropped,
        c.rtx.seen,
        ingress.nacks - nacks_before,
        seq.len()
    );
    assert!(
        (25.0..=35.0).contains(&loss_pct),
        "손실률 {loss_pct:.0}%가 30% 안팎이 아닙니다"
    );
    assert!(
        c.video.dropped >= 80,
        "손실이 적용되지 않았습니다(버린 비디오 패킷 {})",
        c.video.dropped
    );
    assert!(
        ingress.nacks > nacks_before,
        "시청자가 NACK를 보내지 않았습니다"
    );
    assert!(
        c.rtx.seen as f64 >= 0.9 * c.video.dropped as f64,
        "버린 {}개에 재전송이 {}개만 왔습니다",
        c.video.dropped,
        c.rtx.seen
    );
    // 옛 상한(0.15)이었다면 재전송이 지워졌을 상황이고(비율이 0.15를 넘음), 새 상한(0.5)은 넘지 않는다.
    assert!(
        ratio > 0.15 && ratio < 0.5,
        "재전송 비율 최댓값 {ratio:.3}이 0.15~0.5 사이가 아닙니다(손실이 시험 의도와 다릅니다)"
    );
    // 손실 구간의 모든 비디오 프레임이 결국 왔다.
    assert!(
        hs.is_empty(),
        "재전송으로 복구하지 못한 프레임이 있습니다: {hs:?}"
    );
    // 서버 지표(내부 서버): 시청자의 NACK를 받았고 재전송 패킷을 보냈다.
    if let Some(d) = delta(&m0, &metrics_of(&t, &SERIES_B)) {
        eprintln!(
            "[e2e] (b) 서버 지표 변화: 시청자 NACK {} / 재전송 패킷 {}",
            d[0], d[1]
        );
        assert!(d[0] >= 1, "서버가 시청자의 NACK를 세지 않았습니다");
        assert!(
            d[1] as f64 >= 0.9 * c.video.dropped as f64,
            "서버가 보낸 재전송 패킷이 {}개뿐입니다(시청자가 버린 {}개)",
            d[1],
            c.video.dropped
        );
    }

    b.send(ClientMsg::Unpublish);
    drop(w);
    drop(p);
}

// ---------------------------------------------------------------------------
// (c) PLI 재시도 간격과 시청자별 제한

/// (c1) 방송자가 PLI에 답하지 않으면 PLI가 약 500ms마다 오고, 답하기를 켜면 키프레임이 오고 멈춘다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyframe_request_retry_interval_500ms() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "답 없는 방송자").await;
    let id = b.id;
    let p = publish(&mut b, 3000).await;
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = watch(&mut v, id).await;
    let mut rs = Vec::new();
    wait_streaming(&w, &mut rs, 10).await;

    // PLI에 답하지 않게 하고 (a)처럼 끊김을 만든다.
    const SERIES_C1: [&str; 2] = [
        "dongsi_sfu_pli_sent_total",
        "dongsi_sfu_dropped_video_frames_total",
    ];
    let m0 = metrics_of(&t, &SERIES_C1);
    p.set_answer_keyframe_requests(false);
    let started = Instant::now();
    p.impair().drop_for(Classes::VIDEO_AND_RTX, LOSS);
    p.impair().drop_for(Classes::RTX, LOSS + RTX_EXTRA);
    let hold = Duration::from_millis(4500);
    tokio::time::sleep(hold).await;
    let marker = w.viewer.drain();
    rs.extend(marker);
    let all = p.keyframe_requests();
    assert!(
        all.iter().all(|r| r.kind == KeyframeRequestKind::Pli),
        "SFU가 PLI가 아닌 키프레임 요청을 보냈습니다: {all:?}"
    );
    let reqs: Vec<Instant> = all
        .into_iter()
        .filter(|r| r.at >= started)
        .map(|r| r.at)
        .collect();
    let gaps: Vec<u128> = reqs.windows(2).map(|w| ms(w[1] - w[0])).collect();
    eprintln!(
        "[e2e] (c1) 답 없이 {}ms: PLI {}번, 첫 번째 {}ms 뒤, 간격(ms) {gaps:?}",
        ms(hold),
        reqs.len(),
        reqs.first().map_or(0, |r| ms(*r - started))
    );
    assert!(
        reqs.len() >= 5,
        "PLI가 {}번뿐입니다(4.5초 동안 5번 이상이어야 함)",
        reqs.len()
    );
    for g in &gaps {
        assert!(
            (350..=900).contains(g),
            "PLI 간격 {g}ms가 350~900ms를 벗어났습니다: {gaps:?}"
        );
    }
    assert!(
        gaps.iter().all(|g| *g <= 1000),
        "PLI가 1초 넘게 벌어졌습니다: {gaps:?}"
    );

    if let Some(d) = delta(&m0, &metrics_of(&t, &SERIES_C1)) {
        eprintln!(
            "[e2e] (c1) 서버 지표 변화: PLI {} / 버린 비디오 프레임 {}",
            d[0], d[1]
        );
        assert!(
            d[0] >= reqs.len() as u64,
            "서버가 보낸 PLI {}번이 방송자가 받은 {}번보다 적습니다",
            d[0],
            reqs.len()
        );
        assert!(
            d[1] >= 100,
            "서버가 버린 비디오 프레임이 {}개뿐입니다(4.5초 대기)",
            d[1]
        );
    }

    // 키프레임을 기다리는 동안 시청자에게 비디오는 가지 않고(빈틈 뒤 마지막 인덱스 이후 없음) 오디오는 계속 온다.
    let seq = video_seq(&rs);
    let last_video = seq.last().map_or(0, |s| s.0);
    let audio_after = rs
        .iter()
        .filter(|r| r.kind == MediaKind::Audio && r.index.is_some_and(|i| i > last_video))
        .count();
    let sent_tick = p.tick();
    eprintln!(
        "[e2e] (c1) 마지막으로 받은 비디오 tick {last_video}(방송자는 tick {sent_tick}까지 만듦), 그 뒤 오디오 {audio_after}개"
    );
    assert!(
        sent_tick - last_video >= 150,
        "답 없는 동안에도 비디오가 시청자에게 갔습니다(마지막 비디오 tick {last_video}, 방송자 tick {sent_tick})"
    );
    assert!(
        audio_after >= 150,
        "키프레임 대기 동안 오디오가 {audio_after}개뿐입니다"
    );

    // 답하기를 켜면 다음 PLI(0.5초 안)에 키프레임이 오고, 그 뒤로 PLI가 멈춘다.
    let on = Instant::now();
    p.set_answer_keyframe_requests(true);
    let got = collect_until(&w, &mut rs, Duration::from_secs(3), |rs| {
        rs.iter().any(|r| {
            r.kind == MediaKind::Video && r.keyframe && r.index.is_some_and(|i| i > last_video)
        })
    })
    .await;
    let waited = on.elapsed();
    assert!(got, "답하기를 켠 뒤 3초 안에 키프레임이 오지 않았습니다");
    eprintln!(
        "[e2e] (c1) 답하기를 켠 뒤 시청자가 키프레임을 받기까지 {}ms",
        ms(waited)
    );
    assert!(
        waited < Duration::from_millis(1500),
        "키프레임이 {}ms 뒤에 왔습니다",
        ms(waited)
    );
    let key = p
        .keyframes_sent()
        .into_iter()
        .rfind(|k| k.at >= on)
        .expect("키프레임을 보내지 않았습니다");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let late: Vec<u128> = p
        .keyframe_requests()
        .into_iter()
        .filter(|r| r.at > key.at + Duration::from_millis(300))
        .map(|r| ms(r.at - key.at))
        .collect();
    eprintln!("[e2e] (c1) 키프레임 300ms 뒤의 PLI: {late:?}");
    assert!(
        late.is_empty(),
        "키프레임이 온 뒤에도 PLI가 계속 옵니다(키프레임 뒤 ms): {late:?}"
    );

    b.send(ClientMsg::Unpublish);
    drop(w);
    drop(p);
}

/// 시청자 `w`가 `dur` 동안 200ms마다 RTCP PLI와 시그널링 `RequestKeyframe`을 보낸다.
async fn nag(c: &Client, w: &Watching, dur: Duration) {
    let end = Instant::now() + dur;
    while Instant::now() < end {
        w.viewer.request_keyframe();
        c.send(ClientMsg::RequestKeyframe { session: w.session });
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// (c2) 시청자 하나의 키프레임 요청은 방송자에게 2초에 한 번만 닿고, 새 시청자는 그 한도와 상관없이 바로 PLI가 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn viewer_keyframe_requests_limited_per_viewer() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "방송자").await;
    let id = b.id;
    let p = publish(&mut b, 3000).await;
    let mut v1 = Client::connect(&t, "보채는 시청자").await;
    v1.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w1 = watch(&mut v1, id).await;
    let mut rs = Vec::new();
    wait_streaming(&w1, &mut rs, 10).await;
    // 연결 직후의 PLI·키프레임이 가라앉기를 기다린다.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // 1단계: 4.5초 동안 200ms마다 RTCP PLI + RequestKeyframe.
    const SERIES_C2: [&str; 3] = [
        "dongsi_sfu_viewer_pli_fir_received_total",
        "dongsi_sfu_keyframe_requests_ignored_total{source=\"rtcp\"}",
        "dongsi_sfu_keyframe_requests_ignored_total{source=\"signaling\"}",
    ];
    let m0 = metrics_of(&t, &SERIES_C2);
    let base = p.keyframe_requests().len();
    let t1 = Instant::now();
    nag(&v1, &w1, Duration::from_millis(4500)).await;
    let reqs: Vec<Instant> = p.keyframe_requests()[base..].iter().map(|r| r.at).collect();
    let gaps: Vec<u128> = reqs.windows(2).map(|w| ms(w[1] - w[0])).collect();
    let sent = w1.viewer.ingress();
    eprintln!(
        "[e2e] (c2) 4.5초 동안 약 {}번 보챔(시청 쪽 str0m PLI 통계 {}번): 방송자가 받은 키프레임 요청 {}번, 첫 번째 {}ms 뒤, 간격(ms) {gaps:?}",
        4500 / 200,
        sent.plis,
        reqs.len(),
        reqs.first().map_or(0, |r| ms(*r - t1))
    );
    assert!(
        (2..=3).contains(&reqs.len()),
        "시청자 하나의 요청이 방송자에게 {}번 닿았습니다(4.5초에 2~3번이어야 함)",
        reqs.len()
    );
    assert!(
        gaps.iter().all(|g| *g >= 1800),
        "방송자가 받은 키프레임 요청 간격이 2초보다 짧습니다: {gaps:?}"
    );

    if let Some(d) = delta(&m0, &metrics_of(&t, &SERIES_C2)) {
        eprintln!(
            "[e2e] (c2) 서버 지표 변화: 받은 RTCP PLI/FIR {} / 무시한 요청 rtcp {} signaling {}",
            d[0], d[1], d[2]
        );
        assert!(
            d[0] >= 10,
            "서버가 받은 RTCP PLI/FIR이 {}번뿐입니다(시청 쪽 str0m이 PLI를 보내지 않음)",
            d[0]
        );
        assert!(d[1] >= 5, "서버가 무시한 RTCP 요청이 {}번뿐입니다", d[1]);
        assert!(
            d[2] >= 5,
            "서버가 무시한 시그널링 요청이 {}번뿐입니다",
            d[2]
        );
    }

    // 2단계: 시청자 1의 요청이 막혀 있는 동안(방송자에게 닿은 요청 0.3초 뒤부터) 새 시청자가 들어온다.
    let mut seen = p.keyframe_requests().len();
    let wait_end = Instant::now() + Duration::from_secs(3);
    let last = loop {
        nag(&v1, &w1, Duration::from_millis(200)).await;
        let now = p.keyframe_requests();
        if now.len() > seen {
            break now.last().unwrap().at;
        }
        seen = now.len();
        assert!(
            Instant::now() < wait_end,
            "시청자 1의 요청이 3초 안에 다시 닿지 않았습니다"
        );
    };
    nag(&v1, &w1, Duration::from_millis(300)).await;
    let mut v2 = Client::connect(&t, "새 시청자").await;
    v2.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let join_start = Instant::now();
    let before = p.keyframe_requests().len();
    let (w2, join_end) = tokio::join!(
        async {
            let w2 = watch(&mut v2, id).await;
            (w2, Instant::now())
        },
        nag(&v1, &w1, Duration::from_millis(1000)),
    )
    .0;
    let join_after_last = join_end - last;
    assert!(
        join_after_last < Duration::from_millis(1700),
        "새 시청자 연결이 {}ms 걸려 시청자 1의 제한(2초) 안에서 판별할 수 없습니다",
        ms(join_after_last)
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let joined: Vec<u128> = p.keyframe_requests()[before..]
        .iter()
        .filter(|r| r.at >= join_start)
        .map(|r| ms(r.at - join_start))
        .collect();
    eprintln!(
        "[e2e] (c2) 새 시청자 연결({}ms 걸림, 시청자 1의 마지막 요청 뒤 {}ms): 방송자가 받은 PLI(연결 시작 뒤 ms) {joined:?}",
        ms(join_end - join_start),
        ms(join_after_last)
    );
    assert!(
        !joined.is_empty(),
        "시청자 1의 제한이 걸려 있는 동안 새 시청자가 들어왔는데 방송자가 PLI를 받지 못했습니다"
    );

    // 새 시청자는 키프레임을 받는다.
    let mut rs2 = Vec::new();
    let got = collect_until(&w2, &mut rs2, Duration::from_millis(1500), |rs| {
        rs.iter().any(|r| r.kind == MediaKind::Video && r.keyframe)
    })
    .await;
    assert!(got, "새 시청자가 1.5초 안에 키프레임을 받지 못했습니다");

    b.send(ClientMsg::Unpublish);
    drop(w2);
    drop(w1);
    drop(p);
}
