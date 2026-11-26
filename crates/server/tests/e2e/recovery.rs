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
//! 0.15보다 크고 0.5보다 작음을 단언한다(옛 상한이면 지워졌을 상황이었음을 같이 확인한다).
//!
//! 실험(2026-11-23): `sfu.rs`의 `RTX_RATIO_CAP`을 잠시 0.15로 되돌려 이 시험만 한 번 돌렸더니 실패했다(되돌린 뒤
//! `git diff -- crates/server/src`는 비었다). 손실 구간 비디오 원본 396개 중 120개 버림(30%)인데 RTX는 99개만 왔고(0.5일 때
//! 123~126개, 손실 하나당 1번), 시청자 NACK는 32번이었으며, 최근 1초 재전송 비율 최댓값은 0.194(0.5일 때 0.24~0.25)였고
//! 비디오 인덱스 90→106에 빈틈(7프레임)이 생겼다. 첫 실패한 단언은 "재전송 수"(`버린 120개에 재전송이 99개만 왔습니다`)이고
//! 빈틈 단언은 그 뒤라 이때는 평가되지 않았다(빈틈은 출력에서 확인). 곧 이 시험은 상한을 0.15로 되돌리는 회귀를 잡는다.
//! 다만 실행이 한 번뿐이라 빠진 프레임 수는 분포를 모른다.
//!
//! # (c) PLI 재시도 간격과 시청자별 제한
//!
//! - (c1) 방송자가 PLI에 답하지 않으면 SFU는 0.5초마다 PLI를 되풀이한다(`KEYFRAME_RETRY`). 방송자가 받은 PLI 간격이
//!   350~900ms이고 1초를 넘지 않는다(0.5초 + SFU 루프 주기 최대 100ms + 전송·스레드 지연). 그동안 시청자에게 비디오는 가지
//!   않고 오디오만 온다. 답하기를 켜면 키프레임이 오고, 그 뒤로 PLI가 멈춘다.
//! - (c2) 시청자 하나가 200ms마다 RTCP PLI와 시그널링 `RequestKeyframe`을 같이 보내도(같은 시청자 세션의 한도 하나를
//!   공유한다) 방송자가 받는 키프레임 요청은 2초에 한 번 안팎이다(4.5초 동안 2~3번, 간격 1.8초 이상). 그 한도가 걸려 있는
//!   동안 새 시청자가 들어오면 바로 PLI가 간다.
//!
//! # (d) 오디오 RED(RFC 2198): 연속 2개 손실까지 복구한다(거리 `[1, 2]`, `AUDIO_RED_DISTANCES`)
//!
//! 오디오는 NACK가 없어 한 번 잃으면 PLC뿐이다(str0m `format/codec_config.rs:132-136`). SFU는 방송자 세션에서 RED를 풀고
//! 시청자 세션 Writer가 자기 송신 이력으로 다시 감싸므로 방송자→SFU와 SFU→시청자 두 구간이 독립이고 둘 다 켠다
//! (`sfu.rs` 머리 문서 "오디오 RED"). 오디오는 20ms마다 한 패킷(tick 하나)이라, 손실은 `Impair::drop_pattern_for`로
//! 오디오 패킷 20개 중 앞 2개(연속 2개, 손실률 10%)를 2초 동안 버려 5번 넣는다. 오디오 패킷은 RED로 감싸도 PT 하나라
//! `impair.rs`가 `red`를 오디오로 센다. "빠진 프레임"은 시청자가 받은 오디오 tick(`Received::index`)에서 시작 tick부터 받은
//! 가장 큰 tick까지의 빈틈 수다(그 안의 빈틈은 모두 잃은 프레임이다. 복구된 프레임은 순서가 바뀌어 나올 수 있어 순서는 보지
//! 않는다).
//! - (d1) 서버→시청자 손실: RED 시청자는 빠진 프레임이 없고(부하로 인한 별개 손실을 위해 2개까지 허용), 같은 손실을 같은
//!   때 넣은 RED 없는 시청자(`ViewerOptions::red = false`, 대조)는 버린 수만큼 빠진다.
//! - (d2) 방송자→서버 손실: RED 방송자의 시청자는 빠진 프레임이 없고, 같은 손실을 넣은 RED 없는 방송자
//!   (`PendingRawPublisher::with_red(.., false)`, 대조)의 시청자는 버린 수만큼 빠진다.
//! - (d3) 협상과 fallback: 방송자·RED 시청자의 answer SDP에는 `red/48000/2`가 있고 RED를 끈 시청자의 answer에는 없다. RED를
//!   끈 시청자도 오디오를 받는다. 시청자가 받은 오디오 패킷 평균 크기는 RED가 3배쯤이라(패킷 수는 같다) RED 시청자가 2배
//!   넘게 크다(RED가 실제로 선에 실린다는 증거).
//!
//! # (e) 오디오 재정렬 대기(D-092, `sfu.rs` 머리 문서 "오디오 재정렬 대기")
//!
//! SFU 방송자 세션은 seq가 빈 오디오를 완성된 프레임 3개(`AUDIO_REORDER_FRAMES`)까지만 기다린다. 이 시험의 오디오는 20ms
//! 패킷이라 3프레임은 60ms다(앱은 10ms라 30ms).
//! - (e1) RED로도 못 메운 손실: RED 방송자의 오디오를 20개 중 앞 3개씩 2초 동안 버린다. RED [1, 2]가 뒤 2개를 되살리고 첫
//!   1개만 빠진다. 빠진 프레임 앞뒤가 시청자에게 닿은 간격이 200ms 이하다(손실 자체로 80ms, str0m 기본 15프레임이면 약 320ms).
//! - (e2) 순서 뒤바뀜: RED를 끈 방송자의 오디오 패킷 하나를 30ms 늦추기(뒤 패킷 하나가 먼저 감)를 5번 하면 빠지는 프레임이
//!   없다. 비교로 150ms 늦추기(뒤 패킷 7개가 먼저 감)를 5번 한 결과를 출력한다(대기를 넘으므로 빠진다. 단언하지 않음).
//!
//! 각 시험은 4~5초 걸리고 모두 방송자·시청자 쪽 관찰이라 외부 서버에서도 돈다.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use dongsi_core::InstallId;
use dongsi_core::proto::{ClientMsg, ServerMsg};
use str0m::format::Codec;
use str0m::media::{KeyframeRequestKind, MediaKind};

use crate::client::{Client, Watching, find};
use crate::impair::{Class, Classes, LogEntry};
use crate::media::info;
use crate::raw_publisher::{Encoder, PendingRawPublisher, RawOptions, RawPublisher};
use crate::target::Target;
use crate::viewer::{PendingViewer, Received, ViewerOptions, audio_count};

/// 시험용 방송자 표시.
const TAG: u32 = 41;
/// (a)(c1) 방송자가 비디오 원본·RTX를 버리는 시간(비디오 프레임 10개).
const LOSS: Duration = Duration::from_millis(400);
/// (a)(c1) 그 뒤로 RTX만 더 버리는 시간(SFU가 NACK를 포기하고 만료할 때까지, 모듈 설명).
const RTX_EXTRA: Duration = Duration::from_millis(1500);

/// 방송을 시작한다(인코더 흉내). 프레임 페이로드는 `frame_bytes` 바이트쯤.
async fn publish(c: &mut Client, frame_bytes: usize) -> RawPublisher {
    publish_with(c, frame_bytes, true).await.0
}

/// `publish`와 같고, 오디오 RED를 `red`로 정하며 서버의 answer SDP도 돌려준다.
async fn publish_with(c: &mut Client, frame_bytes: usize, red: bool) -> (RawPublisher, String) {
    let pending = PendingRawPublisher::with_red(c.server_ip(), red);
    let req = c.next_req();
    c.send(ClientMsg::Publish {
        req,
        offer: pending.offer.clone(),
        info: info(),
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
    (p, answer)
}

/// 방송을 보고 미디어 연결이 될 때까지 기다린다. 시청 쪽 수신 통계(NACK·PLI 수)를 켠다.
async fn watch(c: &mut Client, broadcaster: InstallId) -> Watching {
    watch_with(c, broadcaster, true).await.0
}

/// `watch`와 같고, 오디오 RED를 `red`로 정하며 서버의 answer SDP도 돌려준다.
async fn watch_with(c: &mut Client, broadcaster: InstallId, red: bool) -> (Watching, String) {
    let pending = PendingViewer::with_options(
        c.server_ip(),
        ViewerOptions {
            log_sr: false,
            stats: true,
            red,
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
    (w, answer)
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

// ---------------------------------------------------------------------------
// (d) 오디오 RED(RFC 2198): 연속 2개 손실까지 복구한다

/// 오디오 손실 시험: 2초 동안 오디오 패킷 20개 중 앞 2개(연속 2개)를 버린다(5번, 10개).
const AUDIO_LOSS: Duration = Duration::from_secs(2);
const AUDIO_BURST: u64 = 2;
const AUDIO_PERIOD: u64 = 20;
/// 이만큼은 손실 주입이 아닌 부하로 빠질 수 있다고 본다(전체 시험이 병렬로 돌 때 오디오가 몇 개 빠진 기록이 있다: (a)의
/// 주석). RED 없이는 버린 수(10개)가 거의 그대로 빠지므로 이 여유로도 RED 유무가 분명히 갈린다.
const RED_MAX_MISSING: usize = 2;

/// 받은 오디오 tick(`Received::index`)의 빈틈 조사 결과.
struct AudioScan {
    /// `lo`(받은 첫 tick보다 작으면 그 tick)부터 받은 가장 큰 tick까지 빠진 tick
    missing: Vec<u32>,
    /// 그 범위에서 받은 서로 다른 tick 수
    received: usize,
    /// 앞 tick보다 작은 tick이 나온 횟수(순서 뒤바뀜)
    out_of_order: usize,
}

fn scan_audio(rs: &[Received], lo: u32) -> AudioScan {
    let audio: Vec<u32> = rs
        .iter()
        .filter(|r| r.kind == MediaKind::Audio)
        .filter_map(|r| r.index)
        .collect();
    let got: BTreeSet<u32> = audio.iter().copied().filter(|i| *i >= lo).collect();
    let (Some(&first), Some(&hi)) = (got.first(), got.last()) else {
        return AudioScan {
            missing: Vec::new(),
            received: 0,
            out_of_order: 0,
        };
    };
    AudioScan {
        missing: (first..=hi).filter(|i| !got.contains(i)).collect(),
        received: got.len(),
        out_of_order: audio.windows(2).filter(|w| w[1] < w[0]).count(),
    }
}

/// 받은 오디오가 모두 Opus로 풀렸다(RED로 받아도 `MediaData`는 Opus 프레임 하나씩이다).
fn all_audio_opus(rs: &[Received]) -> bool {
    rs.iter()
        .filter(|r| r.kind == MediaKind::Audio)
        .all(|r| r.codec == Codec::Opus)
}

/// 시청자가 받은 오디오 패킷 평균 크기(바이트, `impair.rs` 카운터).
fn avg_audio_packet(w: &Watching) -> f64 {
    let c = w.viewer.impair().counters().audio;
    c.bytes as f64 / c.seen.max(1) as f64
}

/// (d1) 서버→시청자 구간 손실: RED 시청자는 빠진 프레임이 없고, RED 없는 시청자(대조)는 버린 수만큼 빠진다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_red_recovers_server_to_viewer_loss() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "오디오 방송자").await;
    let id = b.id;
    let p = publish(&mut b, 3000).await;
    let mut v1 = Client::connect(&t, "RED 시청자").await;
    let mut v2 = Client::connect(&t, "RED 없는 시청자").await;
    v1.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let (red, red_sdp) = watch_with(&mut v1, id, true).await;
    let (plain, plain_sdp) = watch_with(&mut v2, id, false).await;
    assert!(
        red_sdp.contains("red/48000/2"),
        "RED 시청자 answer에 RED가 없습니다"
    );
    assert!(
        !plain_sdp.contains("red/"),
        "RED 없는 시청자 answer에 RED가 있습니다"
    );
    let (mut rs_red, mut rs_plain) = (Vec::new(), Vec::new());
    wait_streaming(&red, &mut rs_red, 10).await;
    wait_streaming(&plain, &mut rs_plain, 10).await;

    // 같은 손실을 두 시청자에게 같은 때 넣는다.
    let lo = p.tick().saturating_sub(5);
    for w in [&red, &plain] {
        w.viewer
            .impair()
            .drop_pattern_for(Classes::AUDIO, AUDIO_BURST, AUDIO_PERIOD, AUDIO_LOSS);
    }
    tokio::time::sleep(AUDIO_LOSS + Duration::from_millis(600)).await;
    rs_red.extend(red.viewer.drain());
    rs_plain.extend(plain.viewer.drain());

    let dropped_red = red.viewer.impair().counters().audio.dropped;
    let dropped_plain = plain.viewer.impair().counters().audio.dropped;
    let (s_red, s_plain) = (scan_audio(&rs_red, lo), scan_audio(&rs_plain, lo));
    eprintln!(
        "[e2e] (d1) 서버→시청자 오디오 연속 {AUDIO_BURST}개씩 {AUDIO_PERIOD}개마다: RED 시청자 버린 {dropped_red}개, 받은 프레임 {}개, 빠진 {}개 {:?}, 순서 뒤바뀜 {} / RED 없는 시청자 버린 {dropped_plain}개, 받은 프레임 {}개, 빠진 {}개 {:?}",
        s_red.received,
        s_red.missing.len(),
        s_red.missing,
        s_red.out_of_order,
        s_plain.received,
        s_plain.missing.len(),
        s_plain.missing
    );
    assert!(
        dropped_red >= 8 && dropped_plain >= 8,
        "손실이 적용되지 않았습니다(버린 오디오 패킷 RED {dropped_red}, 대조 {dropped_plain})"
    );
    assert!(
        all_audio_opus(&rs_red) && all_audio_opus(&rs_plain),
        "받은 오디오가 Opus가 아닙니다"
    );
    assert!(
        s_red.received >= 80,
        "RED 시청자가 오디오 프레임 {}개만 받았습니다",
        s_red.received
    );
    assert!(
        s_red.missing.len() <= RED_MAX_MISSING,
        "RED 시청자의 오디오 프레임이 {}개 빠졌습니다(버린 {dropped_red}개): {:?}",
        s_red.missing.len(),
        s_red.missing
    );
    // 대조: RED가 없으면 버린 패킷이 그대로 빠진다(범위 끝의 몇 개는 뒤 패킷이 없어 세지 않는다).
    assert!(
        s_plain.missing.len() as u64 >= dropped_plain / 2,
        "RED 없는 시청자는 버린 {dropped_plain}개가 빠져야 하는데 {}개만 빠졌습니다(시험이 RED를 보지 못함)",
        s_plain.missing.len()
    );

    b.send(ClientMsg::Unpublish);
    drop(plain);
    drop(red);
    drop(p);
}

/// (d2) 방송자→서버 구간 손실: RED 방송자의 시청자는 빠진 프레임이 없고, RED 없는 방송자(대조)의 시청자는 버린 수만큼 빠진다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_red_recovers_publisher_to_server_loss() {
    let t = Target::shared_with(2).await;
    let mut ba = Client::connect(&t, "RED 방송자").await;
    let mut bb = Client::connect(&t, "RED 없는 방송자").await;
    let (ida, idb) = (ba.id, bb.id);
    let (pa, sdp_a) = publish_with(&mut ba, 3000, true).await;
    let (pb, sdp_b) = publish_with(&mut bb, 3000, false).await;
    assert!(
        sdp_a.contains("red/48000/2"),
        "RED 방송자 answer에 RED가 없습니다"
    );
    assert!(
        !sdp_b.contains("red/"),
        "RED 없는 방송자 answer에 RED가 있습니다(서버가 RED 없는 offer에 RED로 답함)"
    );
    let mut va = Client::connect(&t, "시청자 A").await;
    let mut vb = Client::connect(&t, "시청자 B").await;
    va.wait_list("방송 목록에 내 방송", |l| find(l, ida).is_some())
        .await;
    vb.wait_list("방송 목록에 내 방송", |l| find(l, idb).is_some())
        .await;
    let wa = watch(&mut va, ida).await;
    let wb = watch(&mut vb, idb).await;
    let (mut rs_a, mut rs_b) = (Vec::new(), Vec::new());
    wait_streaming(&wa, &mut rs_a, 10).await;
    wait_streaming(&wb, &mut rs_b, 10).await;

    // 두 방송자가 보내는 오디오 패킷에 같은 손실을 같은 때 넣는다.
    let (lo_a, lo_b) = (pa.tick().saturating_sub(5), pb.tick().saturating_sub(5));
    for p in [&pa, &pb] {
        p.impair()
            .drop_pattern_for(Classes::AUDIO, AUDIO_BURST, AUDIO_PERIOD, AUDIO_LOSS);
    }
    tokio::time::sleep(AUDIO_LOSS + Duration::from_millis(800)).await;
    rs_a.extend(wa.viewer.drain());
    rs_b.extend(wb.viewer.drain());

    let dropped_a = pa.impair().counters().audio.dropped;
    let dropped_b = pb.impair().counters().audio.dropped;
    let (s_a, s_b) = (scan_audio(&rs_a, lo_a), scan_audio(&rs_b, lo_b));
    eprintln!(
        "[e2e] (d2) 방송자→서버 오디오 연속 {AUDIO_BURST}개씩 {AUDIO_PERIOD}개마다: RED 방송자 버린 {dropped_a}개, 시청자가 받은 프레임 {}개, 빠진 {}개 {:?}, 순서 뒤바뀜 {} / RED 없는 방송자 버린 {dropped_b}개, 받은 프레임 {}개, 빠진 {}개 {:?}",
        s_a.received,
        s_a.missing.len(),
        s_a.missing,
        s_a.out_of_order,
        s_b.received,
        s_b.missing.len(),
        s_b.missing
    );
    assert!(
        dropped_a >= 8 && dropped_b >= 8,
        "손실이 적용되지 않았습니다(버린 오디오 패킷 RED {dropped_a}, 대조 {dropped_b})"
    );
    assert!(
        all_audio_opus(&rs_a) && all_audio_opus(&rs_b),
        "받은 오디오가 Opus가 아닙니다"
    );
    assert!(
        s_a.received >= 80,
        "RED 방송자의 시청자가 오디오 프레임 {}개만 받았습니다",
        s_a.received
    );
    assert!(
        s_a.missing.len() <= RED_MAX_MISSING,
        "RED 방송자의 오디오 프레임이 시청자에게 {}개 빠졌습니다(버린 {dropped_a}개): {:?}",
        s_a.missing.len(),
        s_a.missing
    );
    assert!(
        s_b.missing.len() as u64 >= dropped_b / 2,
        "RED 없는 방송자는 버린 {dropped_b}개가 빠져야 하는데 {}개만 빠졌습니다(시험이 RED를 보지 못함)",
        s_b.missing.len()
    );

    ba.send(ClientMsg::Unpublish);
    bb.send(ClientMsg::Unpublish);
    drop(wa);
    drop(wb);
    drop(pa);
    drop(pb);
}

/// (d3) 협상과 fallback: answer SDP의 RED, RED를 끈 시청자의 오디오, RED가 선에 실리는지(패킷 크기).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_red_negotiation_and_fallback() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "방송자").await;
    let id = b.id;
    let (p, pub_sdp) = publish_with(&mut b, 3000, true).await;
    let mut v1 = Client::connect(&t, "RED 시청자").await;
    let mut v2 = Client::connect(&t, "RED 없는 시청자").await;
    v1.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let (red, red_sdp) = watch_with(&mut v1, id, true).await;
    let (plain, plain_sdp) = watch_with(&mut v2, id, false).await;
    eprintln!(
        "[e2e] (d3) answer에 red/48000/2: 방송자 {} / RED 시청자 {} / RED 없는 시청자 {}",
        pub_sdp.contains("red/48000/2"),
        red_sdp.contains("red/48000/2"),
        plain_sdp.contains("red/")
    );
    assert!(
        pub_sdp.contains("red/48000/2"),
        "방송자 answer에 red/48000/2가 없습니다"
    );
    assert!(
        red_sdp.contains("red/48000/2"),
        "RED 시청자 answer에 red/48000/2가 없습니다"
    );
    assert!(
        !plain_sdp.contains("red/"),
        "RED를 끈 시청자 answer에 RED가 있습니다"
    );

    // 두 시청자 모두 오디오 프레임을 60개(1.2초) 넘게 받는다.
    let (mut rs_red, mut rs_plain) = (Vec::new(), Vec::new());
    for (w, rs) in [(&red, &mut rs_red), (&plain, &mut rs_plain)] {
        let ok = collect_until(w, rs, Duration::from_secs(10), |rs| audio_count(rs) >= 60).await;
        assert!(
            ok,
            "시청자가 10초 안에 오디오 프레임 60개를 받지 못했습니다"
        );
    }
    // 시작 부분(연결 직후)은 빼고 두 시청자가 같이 받은 범위에서 본다.
    let lo = [&rs_red, &rs_plain]
        .iter()
        .filter_map(|rs| rs.iter().find(|r| r.kind == MediaKind::Audio)?.index)
        .max()
        .unwrap_or(0);
    let (s_red, s_plain) = (scan_audio(&rs_red, lo), scan_audio(&rs_plain, lo));
    let (size_red, size_plain) = (avg_audio_packet(&red), avg_audio_packet(&plain));
    eprintln!(
        "[e2e] (d3) 받은 오디오 프레임 RED {}개(빠진 {}) / RED 없음 {}개(빠진 {}), 평균 오디오 패킷 RED {size_red:.0}B / RED 없음 {size_plain:.0}B (x{:.2})",
        s_red.received,
        s_red.missing.len(),
        s_plain.received,
        s_plain.missing.len(),
        size_red / size_plain
    );
    assert!(
        all_audio_opus(&rs_red) && all_audio_opus(&rs_plain),
        "받은 오디오가 Opus가 아닙니다"
    );
    // 손실을 넣지 않았으므로 거의 다 온다(부하에서는 몇 개 빠질 수 있다: `MIN_RATIO`와 같은 80%).
    for (who, s) in [("RED", &s_red), ("RED 없는", &s_plain)] {
        let span = s.received + s.missing.len();
        assert!(
            s.received as f64 >= span as f64 * crate::MIN_RATIO,
            "{who} 시청자가 오디오 프레임을 {}/{span}개만 받았습니다",
            s.received
        );
    }
    // RED는 패킷 수는 그대로이고 payload가 (1 + 거리 수)배라 패킷이 2배 넘게 크다(80B 페이로드 + 헤더 약 40B → 약 2.4배).
    assert!(
        size_red > size_plain * 2.0,
        "RED 시청자의 오디오 패킷({size_red:.0}B)이 RED 없는 시청자({size_plain:.0}B)의 2배가 안 됩니다(RED가 선에 실리지 않음)"
    );

    b.send(ClientMsg::Unpublish);
    drop(plain);
    drop(red);
    drop(p);
}

// ---------------------------------------------------------------------------
// (e) 오디오 재정렬 대기(D-092)

/// (e1) RED로도 못 메우는 손실: 오디오 패킷 20개 중 앞 3개(RED [1, 2]는 뒤 2개만 되살린다).
const REORDER_BURST: u64 = 3;
/// (e1) 빠진 프레임 앞뒤가 시청자에게 닿은 간격의 상한. 손실 3패킷만큼 자연히 80ms(4 tick)가 벌어지고, SFU가 3프레임만
/// 기다리면 거기서 끝난다. str0m 기본 15프레임이면 약 320ms(16 tick)다.
const REORDER_GAP_MAX: Duration = Duration::from_millis(200);
/// (e2) 오디오 패킷 하나를 늦추는 시간: 뒤 패킷 하나가 먼저 간다(3프레임 대기 안).
const REORDER_SMALL: Duration = Duration::from_millis(30);
/// (e2) 비교: 뒤 패킷 7개가 먼저 간다(3프레임 대기를 넘는다).
const REORDER_LARGE: Duration = Duration::from_millis(150);
/// (e2) 늦추는 횟수(400ms 간격).
const REORDER_TIMES: usize = 5;

/// 오디오 tick별로 시청자가 처음 받은 시각.
fn audio_arrivals(rs: &[Received]) -> BTreeMap<u32, SystemTime> {
    let mut m = BTreeMap::new();
    for r in rs.iter().filter(|r| r.kind == MediaKind::Audio) {
        if let Some(i) = r.index {
            m.entry(i).or_insert(r.arrived);
        }
    }
    m
}

/// (e1) RED로 못 메운 방송자→서버 오디오 손실 뒤 SFU가 오래 붙잡지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_unrecoverable_loss_held_briefly() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "오디오 손실 방송자").await;
    let id = b.id;
    let p = publish(&mut b, 3000).await;
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = watch(&mut v, id).await;
    let mut rs = Vec::new();
    wait_streaming(&w, &mut rs, 10).await;

    let lo = p.tick().saturating_sub(5);
    p.impair()
        .drop_pattern_for(Classes::AUDIO, REORDER_BURST, AUDIO_PERIOD, AUDIO_LOSS);
    tokio::time::sleep(AUDIO_LOSS + Duration::from_millis(800)).await;
    rs.extend(w.viewer.drain());

    let dropped = p.impair().counters().audio.dropped;
    let scan = scan_audio(&rs, lo);
    let at = audio_arrivals(&rs);
    // 빠진 tick마다 그 앞뒤로 받은 tick이 시청자에게 닿은 간격(ms).
    let gaps: Vec<(u32, u128)> = scan
        .missing
        .iter()
        .filter_map(|&m| {
            let before = at.range(..m).next_back()?.1;
            let after = at.range(m + 1..).next()?.1;
            Some((m, after.duration_since(*before).ok()?.as_millis()))
        })
        .collect();
    eprintln!(
        "[e2e] (e1) 방송자→서버 오디오 연속 {REORDER_BURST}개씩 {AUDIO_PERIOD}개마다: 버린 {dropped}개, 받은 프레임 {}개, 빠진 {:?}, 빠진 곳 앞뒤 도착 간격(tick, ms) {gaps:?}",
        scan.received, scan.missing
    );
    assert!(
        dropped >= 12,
        "손실이 적용되지 않았습니다(버린 오디오 패킷 {dropped})"
    );
    assert!(
        !gaps.is_empty(),
        "RED로 못 메운 손실이 시청자 쪽에 보이지 않습니다(버린 {dropped}개, 빠진 {:?})",
        scan.missing
    );
    let worst = gaps.iter().map(|g| g.1).max().unwrap_or(0);
    assert!(
        worst <= ms(REORDER_GAP_MAX),
        "빠진 오디오 프레임 뒤를 SFU가 {worst}ms 붙잡았습니다(상한 {}ms): {gaps:?}",
        ms(REORDER_GAP_MAX)
    );

    b.send(ClientMsg::Unpublish);
    drop(w);
    drop(p);
}

/// 방송자 오디오 패킷 하나를 `by`만큼 늦추기를 400ms마다 `REORDER_TIMES`번 한다. 그 동안의 (시작 tick, 끝 tick).
/// `RawPublisher::tick`은 소켓에 나간 패킷보다 조금 앞설 수 있어 시작은 3 tick 당겨 잡는다(두 단계 사이는 600ms 띄운다).
async fn delay_audio(p: &RawPublisher, by: Duration) -> (u32, u32) {
    let lo = p.tick().saturating_sub(3);
    for _ in 0..REORDER_TIMES {
        p.impair().delay_next(Classes::AUDIO, 1, by);
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    (lo, p.tick())
}

/// (e2) 방송자→서버 오디오가 조금 뒤바뀌어 오면 SFU가 순서를 맞춰 잃지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_small_reorder_not_lost() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "순서 뒤바뀜 방송자").await;
    let id = b.id;
    // RED를 끈다: RED면 늦은 패킷 자리를 다음 패킷의 사본이 먼저 메워 재정렬 대기를 시험하지 못한다.
    let (p, _) = publish_with(&mut b, 3000, false).await;
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = watch(&mut v, id).await;
    let mut rs = Vec::new();
    wait_streaming(&w, &mut rs, 10).await;

    let small = delay_audio(&p, REORDER_SMALL).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let large = delay_audio(&p, REORDER_LARGE).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    rs.extend(w.viewer.drain());

    let held = p.impair().counters().audio.held;
    let scan = scan_audio(&rs, small.0);
    let missing_in = |(lo, hi): (u32, u32)| {
        scan.missing
            .iter()
            .filter(|m| (lo..hi).contains(*m))
            .count()
    };
    let (miss_small, miss_large) = (missing_in(small), missing_in(large));
    eprintln!(
        "[e2e] (e2) 오디오 패킷 하나씩 {REORDER_TIMES}번 늦춤(붙잡은 {held}개): {}ms → 빠진 {miss_small}개, 비교 {}ms → 빠진 {miss_large}개, 빠진 tick {:?}",
        ms(REORDER_SMALL),
        ms(REORDER_LARGE),
        scan.missing
    );
    assert!(
        held >= 2 * REORDER_TIMES as u64,
        "순서 뒤바뀜이 적용되지 않았습니다(붙잡은 오디오 패킷 {held})"
    );
    assert!(
        miss_small <= RED_MAX_MISSING,
        "{}ms 늦게 온 오디오 패킷을 SFU가 버렸습니다(빠진 {miss_small}개): {:?}",
        ms(REORDER_SMALL),
        scan.missing
    );

    b.send(ClientMsg::Unpublish);
    drop(w);
    drop(p);
}
