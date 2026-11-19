//! 미디어 전 공백이 있을 때의 A/V 동기와, 방송자의 첫 미디어 전에 들어온 시청자의 키프레임 요청.
//!
//! 배경(T3 W2 이상 동작 1·2번): str0m은 미디어를 한 번도 쓰지 않은 송신 스트림도 RTCP SR을 보내는데, 그
//! 내용이 "RTP 0 ↔ 지금 NTP"(`sender_packet_count` 0)다. 이 SR이 시청자에게 가거나 SFU의 캡처 시각 복원에
//! 쓰이면 비디오(RTP 약 10초에서 시작)와 오디오(RTP 0에서 시작)의 시각이 약 10초 어긋난다.
//!
//! 각 시험은 방송 시작·시청자 미디어 연결 뒤 미디어 없이 2초를 두고 3초 동안 펌프를 돌린 다음 본다.
//!
//! - (a) SR이 있는 같은 tick의 비디오·오디오 캡처 시각 차이가 5ms 미만. 단, 트랙이 시작된 직후 방송자의 첫
//!   SR이 오기 전(최대 SR 간격 0.5초)에 나간 프레임은 서버가 도착 시각으로 캡처 시각을 대신하므로
//!   (`sfu.rs` `wallclock`) 시청 쪽의 첫 SR로 복원한 프레임은 전송 지연만큼의 오차를 허용한다(150ms 미만).
//! - (b) 시청자가 받은 SR 가운데 `sender_packet_count`가 0인 것이 없다
//! - (c) 받은 프레임 수가 보낸 수의 `MIN_RATIO` 이상
//! - (d) 캡처 시각이 시청자가 프레임을 받은 시각보다 앞서 있되 2초 안이다(SFU 기준점이 틀어지지 않았다)

use std::time::{Duration, Instant, SystemTime};

use dongsi_core::VideoCodec;
use str0m::media::MediaKind;

use crate::client::{Client, find};
use crate::media::{Pump, Tracks, info};
use crate::target::Target;
use crate::viewer::{Received, SrRecord, audio_count, video_count};
use crate::{MIN_RATIO, settle, viewer_count};

/// 미디어가 시작되기 전 공백 동안 보내는 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gap {
    /// 아무것도 안 보낸다.
    Silent,
    /// 오디오만 보낸다(그다음 오디오+비디오).
    AudioFirst,
    /// 비디오만 보낸다(그다음 오디오+비디오).
    VideoFirst,
}

const GAP_TICKS: u32 = 100; // 2초
const MAIN_TICKS: u32 = 150; // 3초

fn signed_secs(later: SystemTime, earlier: SystemTime) -> f64 {
    match later.duration_since(earlier) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// 시청자가 그 SSRC에서 처음 받은 SR로 캡처 시각을 복원한 프레임인지.
fn from_first_sr(r: &Received, srs: &[SrRecord]) -> bool {
    let Some((ssrc, packets, octets)) = r.sr_key else {
        return false;
    };
    srs.iter()
        .find(|s| s.ssrc == ssrc)
        .is_some_and(|s| s.packets == packets && s.octets == octets)
}

/// 같은 tick의 (비디오 캡처 − 오디오 캡처)(ms, 둘 중 하나라도 첫 SR로 복원했는지). 둘 다 SR로 캡처 시각을
/// 복원한 쌍만 센다.
fn av_pairs(received: &[Received], srs: &[SrRecord]) -> Vec<(u32, f64, bool)> {
    let audio: Vec<&Received> = received
        .iter()
        .filter(|r| r.kind == MediaKind::Audio)
        .collect();
    let mut out = Vec::new();
    for v in received.iter().filter(|r| r.kind == MediaKind::Video) {
        let (Some(i), Some(vc)) = (v.index, v.capture) else {
            continue;
        };
        if let Some(a) = audio.iter().find(|a| a.index == Some(i))
            && let Some(ac) = a.capture
        {
            let early = from_first_sr(v, srs) || from_first_sr(a, srs);
            out.push((i, signed_secs(vc, ac) * 1000.0, early));
        }
    }
    out
}

fn check_av(
    label: &str,
    received: &[Received],
    srs: &[SrRecord],
    sent_video: usize,
    sent_audio: usize,
    origin: SystemTime,
) {
    // 보낸 시각(tick 시각)부터 시청자가 프레임을 받을 때까지(SR과 무관한 실제 지연)
    let mut lat: Vec<(u32, MediaKind, f64)> = received
        .iter()
        .filter_map(|r| {
            let i = r.index?;
            let sent = origin + Duration::from_millis(20 * u64::from(i));
            Some((i, r.kind, signed_secs(r.arrived, sent)))
        })
        .collect();
    let lat_max = lat.iter().fold(f64::MIN, |m, x| m.max(x.2));
    lat.sort_by(|a, b| b.2.total_cmp(&a.2));
    lat.truncate(3);
    eprintln!("[e2e] {label}: 보낸 뒤 받기까지 최대 {lat_max:.3}초 {lat:?}");
    let video = video_count(received);
    let audio = audio_count(received);
    let pairs = av_pairs(received, srs);
    let steady: Vec<&(u32, f64, bool)> = pairs.iter().filter(|p| !p.2).collect();
    let early: Vec<&(u32, f64, bool)> = pairs.iter().filter(|p| p.2).collect();
    let max_diff = steady.iter().fold(0f64, |m, p| m.max(p.1.abs()));
    let max_early = early.iter().fold(0f64, |m, p| m.max(p.1.abs()));
    let zero: Vec<&SrRecord> = srs.iter().filter(|s| s.packets == 0).collect();
    let delays: Vec<f64> = received
        .iter()
        .filter_map(|r| r.capture.map(|c| signed_secs(r.arrived, c)))
        .collect();
    let (min_delay, max_delay) = delays
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(*d), hi.max(*d)));
    let first_sr: Vec<Option<u32>> = received.iter().take(4).map(|r| r.sr_packets).collect();
    if max_delay > 0.1 {
        let mut late: Vec<(Option<u32>, MediaKind, f64, Option<u32>)> = received
            .iter()
            .filter_map(|r| {
                r.capture
                    .map(|c| (r.index, r.kind, signed_secs(r.arrived, c), r.sr_packets))
            })
            .filter(|x| x.2 > 0.1)
            .collect();
        late.truncate(6);
        eprintln!("[e2e] {label}: 지연이 큰 프레임(tick, 종류, 초, SR count): {late:?}");
    }
    eprintln!(
        "[e2e] {label}: 비디오 {video}/{sent_video}, 오디오 {audio}/{sent_audio}, 쌍 {}개(첫 SR {}개), A/V 최대 {max_diff:.2}ms(첫 SR 쌍 {max_early:.2}ms), \
         SR {}개(count 0: {}개), 지연 {min_delay:.3}~{max_delay:.3}s, 처음 프레임들의 SR count {first_sr:?}",
        pairs.len(),
        early.len(),
        srs.len(),
        zero.len(),
    );

    // (c) 프레임 수
    let min_video = (sent_video as f64 * MIN_RATIO) as usize;
    let min_audio = (sent_audio as f64 * MIN_RATIO) as usize;
    assert!(
        video >= min_video,
        "{label}: 비디오 {video}개만 받았습니다(보낸 {sent_video}개 중 {min_video}개 이상이어야 함)"
    );
    assert!(
        audio >= min_audio,
        "{label}: 오디오 {audio}개만 받았습니다(보낸 {sent_audio}개 중 {min_audio}개 이상이어야 함)"
    );

    // (b) 미디어 전 SR이 시청자에게 가지 않는다.
    assert!(
        zero.is_empty(),
        "{label}: sender_packet_count가 0인 SR을 시청자가 {}개 받았습니다: {:?}",
        zero.len(),
        &zero[..zero.len().min(4)]
    );
    assert!(
        received
            .iter()
            .all(|r| r.sr_packets.is_none_or(|n| n > 0)),
        "{label}: 프레임의 캡처 시각을 sender_packet_count 0인 SR로 복원했습니다"
    );

    // (a) A/V 동기. 쌍이 너무 적으면 시험이 의미가 없다.
    assert!(
        pairs.len() >= 25 && steady.len() >= 15,
        "{label}: SR로 짝지은 프레임이 너무 적습니다: {}쌍(첫 SR 제외 {}쌍)",
        pairs.len(),
        steady.len()
    );
    if max_diff >= 5.0 || max_early >= 150.0 {
        let bad: Vec<&(u32, f64, bool)> = pairs
            .iter()
            .filter(|p| p.1.abs() >= if p.2 { 150.0 } else { 5.0 })
            .take(8)
            .collect();
        eprintln!("[e2e] {label}: 큰 쌍(tick, ms, 첫 SR): {bad:?}");
    }
    assert!(max_diff < 5.0, "{label}: A/V 차이가 큽니다: 최대 {max_diff:.2}ms");
    assert!(
        max_early < 150.0,
        "{label}: 방송자의 첫 SR 전에 나간 프레임의 A/V 차이가 큽니다: 최대 {max_early:.2}ms"
    );

    // (d) 캡처 시각의 절대값(SFU 기준점)
    assert!(
        min_delay > -0.05 && max_delay < 2.0,
        "{label}: 캡처 시각이 받은 시각과 너무 다릅니다: {min_delay:.3}~{max_delay:.3}초"
    );
}

async fn scenario(label: &str, gap: Gap) {
    let t = Target::shared().await;
    let mut b = Client::connect(&t, label).await;
    let id = b.id;
    let publisher = b
        .publish(info(VideoCodec::Vp8), None)
        .await
        .expect("방송 시작 실패");

    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = v.watch_logged(id, None).await.expect("시청 실패");
    b.wait_for("ViewerCount 1", viewer_count(1)).await;
    w.viewer.wait_connected(Duration::from_secs(10)).await;
    settle().await;

    // 미디어 전 공백(2초). 시청자가 붙어 있고 방송자의 SR도 이 동안 나간다.
    let mut pump = Pump::new(21);
    let (mut sent_video, mut sent_audio) = (0usize, 0usize);
    match gap {
        Gap::Silent => tokio::time::sleep(Duration::from_millis(u64::from(GAP_TICKS) * 20)).await,
        Gap::AudioFirst => {
            sent_audio += GAP_TICKS as usize;
            pump.run_tracks(&publisher, GAP_TICKS, Tracks::AudioOnly, |_| VideoCodec::Vp8, |_| {})
                .await;
        }
        Gap::VideoFirst => {
            sent_video += pump
                .run_tracks(&publisher, GAP_TICKS, Tracks::VideoOnly, |_| VideoCodec::Vp8, |_| {})
                .await;
        }
    }

    sent_audio += MAIN_TICKS as usize;
    sent_video += pump.vp8(&publisher, MAIN_TICKS).await;
    settle().await;

    let received = w.viewer.drain();
    let srs = w.viewer.sender_reports();
    let origin = SystemTime::now() - pump.origin().expect("펌프가 돌았습니다").elapsed();
    check_av(label, &received, &srs, sent_video, sent_audio, origin);

    b.send(dongsi_core::proto::ClientMsg::Unpublish);
    drop(w);
    publisher.close();
}

/// 시청자가 붙은 뒤 2초 동안 미디어가 없다가 오디오·비디오가 함께 시작한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn av_gap_before_media() {
    scenario("av_gap_before_media", Gap::Silent).await;
}

/// 오디오만 2초 보낸 뒤 비디오가 시작한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn av_audio_first() {
    scenario("av_audio_first", Gap::AudioFirst).await;
}

/// 비디오만 2초 보낸 뒤 오디오가 시작한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn av_video_first() {
    scenario("av_video_first", Gap::VideoFirst).await;
}

/// 방송자의 첫 미디어 전에 들어온 시청자도, 방송자가 미디어를 보내기 시작하면 2초 안에 키프레임을
/// 요청받는다(방송자 SSRC는 첫 RTP를 받아야 생기므로 시청자가 붙은 때의 요청이 사라질 수 있었다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyframe_request_before_first_media() {
    let t = Target::shared().await;
    let mut b = Client::connect(&t, "키프레임 방송자").await;
    let id = b.id;
    // `publish`가 연결 때 켜진 키프레임 요청 표시를 비운다.
    let publisher = b
        .publish(info(VideoCodec::Vp8), None)
        .await
        .expect("방송 시작 실패");

    // 연결 직후의 늦은 표시를 비우고 시작한다.
    tokio::time::sleep(Duration::from_millis(300)).await;
    publisher.take_keyframe_request();
    let base = publisher.stats().keyframe_requests;

    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = v.watch_connected(id, None).await;
    b.wait_for("ViewerCount 1", viewer_count(1)).await;
    settle().await;
    let before_media = publisher.take_keyframe_request();
    eprintln!(
        "[e2e] 키프레임 요청: 미디어 전에 받음 = {before_media}(요청 수 {})",
        publisher.stats().keyframe_requests - base
    );

    // 미디어를 시작한다. 2초 안에 방송자가 키프레임 요청을 받아야 한다.
    w.viewer.drain();
    let started = Instant::now();
    let mut pump = Pump::new(22);
    let mut got_at: Option<Duration> = before_media.then_some(Duration::ZERO);
    let sent_video = pump
        .run(
            &publisher,
            100,
            |_| VideoCodec::Vp8,
            |_| {
                if got_at.is_none() && publisher.take_keyframe_request() {
                    got_at = Some(started.elapsed());
                }
            },
        )
        .await;
    if got_at.is_none() && publisher.take_keyframe_request() {
        got_at = Some(started.elapsed());
    }
    eprintln!(
        "[e2e] 키프레임 요청: 미디어 시작 {got_at:?} 뒤 받음(요청 수 {})",
        publisher.stats().keyframe_requests - base
    );
    assert!(
        got_at.is_some_and(|d| d < Duration::from_secs(2)),
        "미디어를 보내기 시작한 뒤 2초 안에 키프레임 요청이 오지 않았습니다"
    );
    settle().await;

    // 시청자는 첫 비디오부터 받는다(시험 미디어는 모두 키프레임이다).
    let received = w.viewer.drain();
    let video = video_count(&received);
    assert!(
        video >= (sent_video as f64 * MIN_RATIO) as usize,
        "비디오 {video}개만 받았습니다(보낸 {sent_video}개)"
    );

    b.send(dongsi_core::proto::ClientMsg::Unpublish);
    drop(w);
    publisher.close();
}

/// 방송자가 키프레임을 보내지 않으면 서버는 PLI를 되풀이하되 1초에 한 번을 넘기지 않고, 키프레임이 오면
/// 멈춘다(PLI가 유실돼도 시청자가 키프레임을 못 받은 채 남지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyframe_request_retried_until_keyframe() {
    let t = Target::shared().await;
    let mut b = Client::connect(&t, "재요청 방송자").await;
    let id = b.id;
    let publisher = b
        .publish(info(VideoCodec::Vp8), None)
        .await
        .expect("방송 시작 실패");
    tokio::time::sleep(Duration::from_millis(300)).await;
    publisher.take_keyframe_request();

    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = v.watch_connected(id, None).await;
    b.wait_for("ViewerCount 1", viewer_count(1)).await;
    settle().await;
    let base = publisher.stats().keyframe_requests;

    // 델타 프레임만 3.5초. 서버는 키프레임을 기다리며 PLI를 1초마다 되풀이한다.
    let mut pump = Pump::new(23);
    pump.set_keyframes(false);
    let started = Instant::now();
    pump.vp8(&publisher, 175).await;
    let waiting = publisher.stats().keyframe_requests - base;
    let secs = started.elapsed().as_secs_f64();
    eprintln!("[e2e] 키프레임 없이 {secs:.1}초: PLI {waiting}번");
    assert!(
        waiting >= 3,
        "키프레임이 오지 않는데 PLI가 {waiting}번뿐입니다({secs:.1}초)"
    );
    assert!(
        waiting as f64 <= secs.ceil() + 1.0,
        "PLI가 너무 잦습니다: {secs:.1}초에 {waiting}번"
    );

    // 키프레임이 오면 멈춘다(오가는 중이던 요청 하나는 허용).
    pump.set_keyframes(true);
    pump.vp8(&publisher, 25).await;
    settle().await;
    let after_key = publisher.stats().keyframe_requests - base;
    pump.set_keyframes(false);
    pump.vp8(&publisher, 125).await; // 2.5초 더(델타 프레임)
    let end = publisher.stats().keyframe_requests - base;
    eprintln!("[e2e] 키프레임 뒤: PLI {after_key}번 → {end}번");
    assert!(
        end <= after_key + 1,
        "키프레임을 받은 뒤에도 PLI가 계속 갑니다: {after_key}번 → {end}번"
    );

    b.send(dongsi_core::proto::ClientMsg::Unpublish);
    drop(w);
    publisher.close();
}
