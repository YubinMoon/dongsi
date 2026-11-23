//! simulcast 트랙 고르기(D-102, 규칙은 D-098부터): SFU가 시청자마다 받을 트랙(상 = 방송 화질, 하 = FHD 3Mbps)을 고른다.
//!
//! - 고르는 규칙: 트랙 비트레이트가 시청자 세션 BWE 추정값 × 0.9 이하인 트랙 가운데 비트레이트가 가장 높은 것. 하나도
//!   없으면 비트레이트가 가장 낮은 것. 추정값이 아직 없으면 비트레이트가 가장 높은 것(시작 추정값이 그보다 높게 잡혀 있다,
//!   `sfu.rs`). 순서는 늘 비트레이트로 본다: 방송자 업로드가 모자라 상이 3000 아래로 내려가면 하가 "더 높은" 트랙이다.
//!   같으면 상을 높게 본다.
//! - 트랙 비트레이트는 방송자가 알린 목표(`ClientMsg::PublishLayers`)다. 아직 안 왔으면 SFU가 잰 받는 비트레이트를 쓴다.
//!   목표가 0인 트랙(하가 쉰다: 설정 3Mbps 이하, 소프트웨어 인코더 코덱)과 다른 트랙보다 [`LAYER_IDLE`] 넘게 프레임이 안 온
//!   트랙(하 인코더 실패 등)은 고르지 않는다.
//! - 바꾸기: 내릴 때(또는 지금 트랙이 사라졌을 때)는 바로, 올릴 때는 [`UP_HOLD`] 동안 줄곧 올릴 수 있어야 한다. 바꿀
//!   트랙의 키프레임을 요청하고, 그 키프레임이 올 때까지는 지금 트랙을 그대로 보낸다(멈춤 없이 이어진다). 두 트랙의 RTP
//!   시각은 같은 캡처 시각에서 나와서 시청자 쪽 시각도 이어진다.

use std::cmp::Reverse;
use std::time::{Duration, Instant};

use dongsi_core::{Layer, LayerRates};

/// 추정값 중 비디오 트랙에 쓰는 비율(D-098: 추정 × 0.9 ≥ 트랙 비트레이트).
pub(crate) const HEADROOM: f64 = 0.9;
/// 더 높은 트랙으로 올리기 전에 줄곧 올릴 수 있어야 하는 시간.
pub(crate) const UP_HOLD: Duration = Duration::from_secs(4);
/// 다른 트랙 프레임은 오는데 이만큼 넘게 프레임이 안 온 트랙은 쉬는 것으로 본다(하 인코더를 못 염 등).
/// 화면이 멈춰 모든 트랙이 같이 쉬면 서로 뒤처지지 않으므로 그대로다.
pub(crate) const LAYER_IDLE: Duration = Duration::from_millis(1500);

/// 트랙마다(`Layer::index`) 고를 때 쓸 비트레이트(kbps). 고를 수 없는 트랙은 `None`.
pub(crate) type Avail = [Option<u32>; Layer::ALL.len()];

/// 방송자 세션 쪽 트랙 상태.
#[derive(Debug, Default)]
pub(crate) struct PublisherLayers {
    /// 방송자가 알린 트랙 목표(kbps). 아직 안 왔으면 `None`.
    rates: Option<LayerRates>,
    /// 트랙마다 마지막 비디오 프레임이 온 때.
    last_frame: [Option<Instant>; Layer::ALL.len()],
}

impl PublisherLayers {
    pub fn set_rates(&mut self, rates: LayerRates) {
        self.rates = Some(rates);
    }

    pub fn rates(&self) -> Option<LayerRates> {
        self.rates
    }

    pub fn on_frame(&mut self, layer: Layer, now: Instant) {
        self.last_frame[layer.index()] = Some(now);
    }

    /// 트랙마다 고를 때 쓸 비트레이트(kbps). 고를 수 없는 트랙은 `None`. `measured`는 SFU가 잰 트랙별 받는
    /// 비트레이트로, 방송자가 목표를 알리기 전에만 쓴다.
    pub fn available(&self, measured: Avail) -> Avail {
        let newest = self.last_frame.iter().flatten().max().copied();
        std::array::from_fn(|i| {
            let seen = self.last_frame[i]?;
            if newest.is_some_and(|n| n.saturating_duration_since(seen) > LAYER_IDLE) {
                return None;
            }
            match self.rates {
                Some(r) => (r[i] > 0).then_some(r[i]),
                None => measured[i],
            }
        })
    }
}

/// 트랙의 순서 열쇠: 비트레이트가 높을수록, 같으면 상이 크다. 고를 수 없는 트랙은 `None`.
fn rank(avail: &Avail, l: Layer) -> Option<(u32, Reverse<usize>)> {
    avail[l.index()].map(|k| (k, Reverse(l.index())))
}

/// 고를 수 있는 트랙을 높은 순서로(모듈 설명).
fn by_rank(avail: &Avail) -> Vec<Layer> {
    let mut v: Vec<Layer> = Layer::ALL
        .into_iter()
        .filter(|l| avail[l.index()].is_some())
        .collect();
    v.sort_by_key(|l| Reverse(rank(avail, *l)));
    v
}

/// 고를 수 있는 트랙 가운데 추정값에 맞는 것(모듈 설명). 고를 수 있는 트랙이 없으면 `None`.
pub(crate) fn best_layer(estimate_kbps: Option<u32>, avail: &Avail) -> Option<Layer> {
    let active = by_rank(avail);
    let Some(estimate) = estimate_kbps else {
        return active.first().copied();
    };
    let budget = estimate as f64 * HEADROOM;
    active
        .iter()
        .find(|l| avail[l.index()].is_some_and(|k| k as f64 <= budget))
        .or(active.last())
        .copied()
}

/// `a`가 `b`보다 높은 트랙인지(비트레이트 순서). 둘 다 고를 수 있어야 한다.
fn higher(a: Layer, b: Layer, avail: &Avail) -> bool {
    rank(avail, a) > rank(avail, b)
}

/// 시청자 하나의 트랙 상태.
#[derive(Debug, Default)]
pub(crate) struct LayerPicker {
    /// 지금 보내는 트랙. 첫 키프레임 전에는 `None`.
    current: Option<Layer>,
    /// 바꾸려는 트랙. 그 트랙의 키프레임이 오면 바꾼다.
    target: Option<Layer>,
    /// 더 높은 트랙으로 올릴 수 있게 된 때(올림 대기).
    up_since: Option<Instant>,
}

/// [`LayerPicker::on_frame`]의 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameStep {
    /// 이 시청자에게 보내지 않는다(다른 트랙).
    Skip,
    /// 보낸다.
    Forward,
    /// 바꾸려던 트랙의 키프레임이라 이 프레임부터 그 트랙을 보낸다(`from`은 이전 트랙).
    Switched { from: Option<Layer> },
}

impl LayerPicker {
    pub fn current(&self) -> Option<Layer> {
        self.current
    }

    /// 시청자가 키프레임을 요청했을 때 방송자에게 요청할 트랙: 바꾸는 중이면 바꿀 트랙, 아니면 지금 트랙.
    pub fn keyframe_layer(&self) -> Option<Layer> {
        self.target.or(self.current)
    }

    /// 첫 트랙을 정한다(시청자 연결 때). 이미 정했으면 그대로 둔다. 키프레임을 요청할 트랙을 돌려준다.
    pub fn start(&mut self, best: Option<Layer>) -> Layer {
        if let Some(l) = self.keyframe_layer() {
            return l;
        }
        let l = best.unwrap_or(Layer::High);
        self.target = Some(l);
        l
    }

    /// 추정값과 트랙 상태를 보고 바꿀 트랙을 정한다. 새로 바꾸기로 했으면 그 트랙(키프레임을 요청할 트랙)을 돌려준다.
    pub fn update(
        &mut self,
        now: Instant,
        estimate_kbps: Option<u32>,
        avail: &Avail,
    ) -> Option<Layer> {
        let Some(best) = best_layer(estimate_kbps, avail) else {
            self.up_since = None;
            return None;
        };
        let Some(cur) = self.current else {
            // 아직 첫 키프레임 전: 바로 고른다.
            self.up_since = None;
            return self.retarget(best);
        };
        if best == cur {
            // 지금 트랙이 맞다. 바꾸던 것을 그만둔다.
            self.target = None;
            self.up_since = None;
            return None;
        }
        if avail[cur.index()].is_none() || !higher(best, cur, avail) {
            // 내림, 또는 지금 트랙이 사라졌다: 바로.
            self.up_since = None;
            return self.retarget(best);
        }
        // 올림. 이미 더 높이 올리려던 중이면(또는 올리려던 트랙이 사라졌으면) 낮추는 것은 바로 한다.
        if let Some(t) = self.target
            && (avail[t.index()].is_none() || higher(t, best, avail))
        {
            return self.retarget(best);
        }
        if self.target == Some(best) {
            return None;
        }
        let since = *self.up_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= UP_HOLD {
            self.up_since = None;
            return self.retarget(best);
        }
        None
    }

    fn retarget(&mut self, layer: Layer) -> Option<Layer> {
        if self.target == Some(layer) {
            return None;
        }
        self.target = Some(layer);
        Some(layer)
    }

    /// `layer`의 비디오 프레임을 이 시청자에게 보낼지. 바꾸려는 트랙의 키프레임이면 이 프레임부터 바꾼다.
    pub fn on_frame(&mut self, layer: Layer, keyframe: bool) -> FrameStep {
        if self.target == Some(layer) && keyframe {
            let from = self.current.replace(layer);
            self.target = None;
            return FrameStep::Switched { from };
        }
        if self.current == Some(layer) {
            FrameStep::Forward
        } else {
            FrameStep::Skip
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: Avail = [Some(8000), Some(3000)];

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn best_layer_fits_the_estimate() {
        // 추정 × 0.9 ≥ 트랙 비트레이트인 가장 높은 트랙
        assert_eq!(best_layer(Some(9000), &BOTH), Some(Layer::High)); // 8100
        assert_eq!(best_layer(Some(8888), &BOTH), Some(Layer::Low)); // 7999
        assert_eq!(best_layer(Some(3334), &BOTH), Some(Layer::Low)); // 3000.6
        // 하에도 모자라면 가장 낮은 트랙
        assert_eq!(best_layer(Some(500), &BOTH), Some(Layer::Low));
        // 추정값이 없으면 가장 높은 트랙
        assert_eq!(best_layer(None, &BOTH), Some(Layer::High));
        // 쉬는 하는 고르지 않는다(단일).
        let high_only = [Some(2500), None];
        assert_eq!(best_layer(Some(500), &high_only), Some(Layer::High));
        assert_eq!(best_layer(None, &high_only), Some(Layer::High));
        assert_eq!(best_layer(Some(500), &[None; 2]), None);
    }

    /// 업로드가 모자라 상이 하(3000)보다 낮아지면 순서가 뒤집힌다: 하가 더 높은 트랙이다.
    #[test]
    fn order_follows_the_bitrate_not_the_rid() {
        let low_high = [Some(1500), Some(3000)];
        assert_eq!(best_layer(None, &low_high), Some(Layer::Low));
        assert_eq!(best_layer(Some(50_000), &low_high), Some(Layer::Low));
        // 하에 모자라면 비트레이트가 더 낮은 상
        assert_eq!(best_layer(Some(3000), &low_high), Some(Layer::High)); // 2700
        assert_eq!(best_layer(Some(500), &low_high), Some(Layer::High));
        // 같으면 상을 높게 본다.
        let tie = [Some(3000), Some(3000)];
        assert_eq!(best_layer(Some(50_000), &tie), Some(Layer::High));
        assert_eq!(best_layer(Some(100), &tie), Some(Layer::Low));
        assert!(higher(Layer::Low, Layer::High, &low_high));
        assert!(!higher(Layer::Low, Layer::High, &BOTH));
    }

    #[test]
    fn publisher_layers_hide_paused_and_stalled_tracks() {
        let t0 = Instant::now();
        let mut p = PublisherLayers::default();
        let measured = [Some(7000), Some(2800)];
        // 프레임이 한 번도 안 온 트랙은 고르지 않는다.
        assert_eq!(p.available(measured), [None; 2]);
        for l in Layer::ALL {
            p.on_frame(l, t0);
        }
        // 방송자가 목표를 알리기 전에는 잰 값
        assert_eq!(p.available(measured), measured);
        // 하 목표 0(쉼)
        p.set_rates([2500, 0]);
        assert_eq!(p.available(measured), [Some(2500), None]);
        // 하만 1.5초 넘게 프레임이 안 오면 뺀다. 모두 같이 멈추면(화면 멈춤) 그대로다.
        p.set_rates([8000, 3000]);
        p.on_frame(Layer::High, t0 + s(2));
        assert_eq!(p.available(measured), [Some(8000), None]);
        p.on_frame(Layer::Low, t0 + s(2));
        assert_eq!(p.available(measured), BOTH);
    }

    #[test]
    fn picker_switches_on_the_target_keyframe() {
        let mut p = LayerPicker::default();
        assert_eq!(p.start(Some(Layer::High)), Layer::High);
        assert_eq!(
            p.start(Some(Layer::Low)),
            Layer::High,
            "이미 정했으면 그대로"
        );
        // 키프레임 전에는 보내지 않는다.
        assert_eq!(p.on_frame(Layer::High, false), FrameStep::Skip);
        assert_eq!(p.on_frame(Layer::Low, true), FrameStep::Skip);
        assert_eq!(
            p.on_frame(Layer::High, true),
            FrameStep::Switched { from: None }
        );
        assert_eq!(p.current(), Some(Layer::High));
        assert_eq!(p.on_frame(Layer::High, false), FrameStep::Forward);
        assert_eq!(p.on_frame(Layer::Low, false), FrameStep::Skip);
    }

    #[test]
    fn picker_goes_down_at_once_and_up_after_holding() {
        let t0 = Instant::now();
        let mut p = LayerPicker::default();
        p.start(Some(Layer::High));
        p.on_frame(Layer::High, true);
        assert_eq!(p.update(t0, Some(9000), &BOTH), None);
        // 내림은 바로 정하고, 하 키프레임이 올 때까지 상을 그대로 보낸다.
        assert_eq!(p.update(t0, Some(5000), &BOTH), Some(Layer::Low));
        assert_eq!(p.update(t0, Some(5000), &BOTH), None, "한 번만 요청");
        assert_eq!(p.keyframe_layer(), Some(Layer::Low));
        assert_eq!(p.on_frame(Layer::High, false), FrameStep::Forward);
        assert_eq!(p.on_frame(Layer::Low, false), FrameStep::Skip);
        assert_eq!(
            p.on_frame(Layer::Low, true),
            FrameStep::Switched {
                from: Some(Layer::High)
            }
        );
        assert_eq!(p.on_frame(Layer::High, false), FrameStep::Skip);
        // 올림은 4초 동안 줄곧 올릴 수 있어야 한다.
        assert_eq!(p.update(t0 + s(1), Some(9000), &BOTH), None);
        assert_eq!(p.update(t0 + s(4), Some(9000), &BOTH), None);
        // 중간에 떨어지면 다시 센다.
        assert_eq!(p.update(t0 + s(4), Some(2000), &BOTH), None);
        assert_eq!(p.update(t0 + s(5), Some(9000), &BOTH), None);
        assert_eq!(p.update(t0 + s(8), Some(9000), &BOTH), None);
        assert_eq!(p.update(t0 + s(9), Some(9000), &BOTH), Some(Layer::High));
        assert_eq!(p.keyframe_layer(), Some(Layer::High));
        // 지금 트랙(하)이 맞게 되면 바꾸던 것을 그만둔다.
        assert_eq!(p.update(t0 + s(10), Some(5000), &BOTH), None);
        assert_eq!(p.keyframe_layer(), Some(Layer::Low));
        assert_eq!(p.on_frame(Layer::High, true), FrameStep::Skip);
    }

    /// 상을 보던 시청자: 방송자가 업로드가 모자라 상을 하보다 낮추면 하가 "올림"이라 4초 뒤 하로 간다. 추정이 하에
    /// 모자라면 상(이제 가장 낮은 트랙)에 머문다.
    #[test]
    fn viewer_moves_to_low_when_high_falls_below_it() {
        let t0 = Instant::now();
        let mut p = LayerPicker::default();
        p.start(Some(Layer::High));
        p.on_frame(Layer::High, true);
        let squeezed = [Some(1500), Some(3000)];
        assert_eq!(p.update(t0, Some(50_000), &squeezed), None);
        assert_eq!(p.update(t0 + s(3), Some(50_000), &squeezed), None);
        assert_eq!(
            p.update(t0 + s(4), Some(50_000), &squeezed),
            Some(Layer::Low)
        );
        let mut q = LayerPicker::default();
        q.start(Some(Layer::High));
        q.on_frame(Layer::High, true);
        assert_eq!(q.update(t0, Some(2000), &squeezed), None);
        // 하를 보던 시청자는 상이 하보다 낮아도 하에 머문다(내림이 아니다).
        let mut r = LayerPicker::default();
        r.start(Some(Layer::Low));
        r.on_frame(Layer::Low, true);
        assert_eq!(r.update(t0, Some(50_000), &squeezed), None);
        // 하가 맞지 않게 되면 상으로 바로 내린다.
        assert_eq!(r.update(t0, Some(3000), &squeezed), Some(Layer::High));
    }

    /// 하를 보던 시청자는 하가 쉬면(설정을 3Mbps 이하로, 소프트웨어 코덱) 바로 상으로 간다(올림이어도 기다리지 않는다).
    #[test]
    fn picker_leaves_a_paused_track_at_once() {
        let t0 = Instant::now();
        let mut p = LayerPicker::default();
        p.start(Some(Layer::Low));
        p.on_frame(Layer::Low, true);
        let high_only = [Some(2500), None];
        assert_eq!(p.update(t0, Some(1000), &high_only), Some(Layer::High));
        // 하가 돌아와도 상이 추정에 맞으면 상 그대로다.
        p.on_frame(Layer::High, true);
        assert_eq!(p.update(t0 + s(1), Some(50_000), &BOTH), None);
        // 올리려던 트랙(상)이 키프레임 전에 사라지면 바꾸기를 그만두고 지금 트랙(하)에 머문다.
        let mut q = LayerPicker::default();
        q.start(Some(Layer::Low));
        q.on_frame(Layer::Low, true);
        assert_eq!(q.update(t0, Some(50_000), &BOTH), None);
        assert_eq!(q.update(t0 + s(4), Some(50_000), &BOTH), Some(Layer::High));
        assert_eq!(q.keyframe_layer(), Some(Layer::High));
        assert_eq!(q.update(t0 + s(4), Some(50_000), &[None, Some(3000)]), None);
        assert_eq!(q.keyframe_layer(), Some(Layer::Low));
        assert_eq!(q.on_frame(Layer::High, true), FrameStep::Skip);
    }
}
