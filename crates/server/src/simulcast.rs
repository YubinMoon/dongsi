//! simulcast 계층 고르기(D-098): SFU가 시청자마다 받을 계층(상·중·하)을 고른다.
//!
//! - 고르는 규칙: 계층 비트레이트가 시청자 세션 BWE 추정값 × 0.9 이하인 가장 높은 계층. 하나도 없으면 가장 낮은
//!   계층. 추정값이 아직 없으면 가장 높은 계층(시작 추정값이 상 계층보다 높게 잡혀 있다, `sfu.rs`).
//! - 계층 비트레이트는 방송자가 알린 목표(`ClientMsg::PublishLayers`)다. 아직 안 왔으면 SFU가 잰 받는 비트레이트를 쓴다.
//!   목표가 0인 계층(방송자가 업로드가 모자라 뺐다)과 다른 계층보다 [`LAYER_IDLE`] 넘게 프레임이 안 온 계층(계층
//!   인코더 실패 등)은 고르지 않는다.
//! - 바꾸기: 내릴 때(또는 지금 계층이 사라졌을 때)는 바로, 올릴 때는 [`UP_HOLD`] 동안 줄곧 올릴 수 있어야 한다. 바꿀
//!   계층의 키프레임을 요청하고, 그 키프레임이 올 때까지는 지금 계층을 그대로 보낸다(멈춤 없이 이어진다). 세 계층의 RTP
//!   시각은 같은 캡처 시각에서 나와서 시청자 쪽 시각도 이어진다.

use std::time::{Duration, Instant};

use dongsi_core::{Layer, LayerRates};

/// 추정값 중 비디오 계층에 쓰는 비율(D-098: 추정 × 0.9 ≥ 계층 비트레이트).
pub(crate) const HEADROOM: f64 = 0.9;
/// 더 높은 계층으로 올리기 전에 줄곧 올릴 수 있어야 하는 시간.
pub(crate) const UP_HOLD: Duration = Duration::from_secs(4);
/// 다른 계층 프레임은 오는데 이만큼 넘게 프레임이 안 온 계층은 쉬는 것으로 본다(계층 인코더를 못 염 등).
/// 화면이 멈춰 모든 계층이 같이 쉬면 서로 뒤처지지 않으므로 그대로다.
pub(crate) const LAYER_IDLE: Duration = Duration::from_millis(1500);

/// 방송자 세션 쪽 계층 상태.
#[derive(Debug, Default)]
pub(crate) struct PublisherLayers {
    /// 방송자가 알린 계층 목표(kbps). 아직 안 왔으면 `None`.
    rates: Option<LayerRates>,
    /// 계층마다 마지막 비디오 프레임이 온 때.
    last_frame: [Option<Instant>; 3],
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

    /// 계층마다 고를 때 쓸 비트레이트(kbps). 고를 수 없는 계층은 `None`. `measured`는 SFU가 잰 계층별 받는
    /// 비트레이트로, 방송자가 목표를 알리기 전에만 쓴다.
    pub fn available(&self, measured: [Option<u32>; 3]) -> [Option<u32>; 3] {
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

/// 고를 수 있는 계층 가운데 추정값에 맞는 것(모듈 설명). 고를 수 있는 계층이 없으면 `None`.
pub(crate) fn best_layer(estimate_kbps: Option<u32>, avail: &[Option<u32>; 3]) -> Option<Layer> {
    let active = || {
        Layer::ALL
            .into_iter()
            .filter(|l| avail[l.index()].is_some())
    };
    let Some(estimate) = estimate_kbps else {
        return active().next();
    };
    let budget = estimate as f64 * HEADROOM;
    active()
        .find(|l| avail[l.index()].is_some_and(|k| k as f64 <= budget))
        .or_else(|| active().next_back())
}

/// `a`가 `b`보다 화질이 높은 계층인지.
fn higher(a: Layer, b: Layer) -> bool {
    a.index() < b.index()
}

/// 시청자 하나의 계층 상태.
#[derive(Debug, Default)]
pub(crate) struct LayerPicker {
    /// 지금 보내는 계층. 첫 키프레임 전에는 `None`.
    current: Option<Layer>,
    /// 바꾸려는 계층. 그 계층의 키프레임이 오면 바꾼다.
    target: Option<Layer>,
    /// 더 높은 계층으로 올릴 수 있게 된 때(올림 대기).
    up_since: Option<Instant>,
}

/// [`LayerPicker::on_frame`]의 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameStep {
    /// 이 시청자에게 보내지 않는다(다른 계층).
    Skip,
    /// 보낸다.
    Forward,
    /// 바꾸려던 계층의 키프레임이라 이 프레임부터 그 계층을 보낸다(`from`은 이전 계층).
    Switched { from: Option<Layer> },
}

impl LayerPicker {
    pub fn current(&self) -> Option<Layer> {
        self.current
    }

    /// 시청자가 키프레임을 요청했을 때 방송자에게 요청할 계층: 바꾸는 중이면 바꿀 계층, 아니면 지금 계층.
    pub fn keyframe_layer(&self) -> Option<Layer> {
        self.target.or(self.current)
    }

    /// 첫 계층을 정한다(시청자 연결 때). 이미 정했으면 그대로 둔다. 키프레임을 요청할 계층을 돌려준다.
    pub fn start(&mut self, best: Option<Layer>) -> Layer {
        if let Some(l) = self.keyframe_layer() {
            return l;
        }
        let l = best.unwrap_or(Layer::High);
        self.target = Some(l);
        l
    }

    /// 추정값과 계층 상태를 보고 바꿀 계층을 정한다. 새로 바꾸기로 했으면 그 계층(키프레임을 요청할 계층)을 돌려준다.
    pub fn update(
        &mut self,
        now: Instant,
        estimate_kbps: Option<u32>,
        avail: &[Option<u32>; 3],
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
            // 지금 계층이 맞다. 바꾸던 것을 그만둔다.
            self.target = None;
            self.up_since = None;
            return None;
        }
        if !higher(best, cur) || avail[cur.index()].is_none() {
            // 내림, 또는 지금 계층이 사라졌다: 바로.
            self.up_since = None;
            return self.retarget(best);
        }
        // 올림. 이미 더 높이 올리려던 중이면 낮추는 것은 바로 한다.
        if let Some(t) = self.target
            && higher(t, best)
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

    /// `layer`의 비디오 프레임을 이 시청자에게 보낼지. 바꾸려는 계층의 키프레임이면 이 프레임부터 바꾼다.
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

    const ALL: [Option<u32>; 3] = [Some(8000), Some(4000), Some(2000)];

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn best_layer_fits_the_estimate() {
        // 추정 × 0.9 ≥ 계층 비트레이트인 가장 높은 계층
        assert_eq!(best_layer(Some(9000), &ALL), Some(Layer::High)); // 8100
        assert_eq!(best_layer(Some(8888), &ALL), Some(Layer::Mid)); // 7999
        assert_eq!(best_layer(Some(4445), &ALL), Some(Layer::Mid)); // 4000.5
        assert_eq!(best_layer(Some(4000), &ALL), Some(Layer::Low)); // 3600
        // 하에도 모자라면 가장 낮은 계층
        assert_eq!(best_layer(Some(500), &ALL), Some(Layer::Low));
        // 추정값이 없으면 가장 높은 계층
        assert_eq!(best_layer(None, &ALL), Some(Layer::High));
        // 뺀 계층은 고르지 않는다.
        let no_low = [Some(8000), Some(4000), None];
        assert_eq!(best_layer(Some(500), &no_low), Some(Layer::Mid));
        let high_only = [Some(8000), None, None];
        assert_eq!(best_layer(Some(500), &high_only), Some(Layer::High));
        assert_eq!(best_layer(Some(500), &[None; 3]), None);
    }

    #[test]
    fn publisher_layers_hide_dropped_and_stalled_layers() {
        let t0 = Instant::now();
        let mut p = PublisherLayers::default();
        let measured = [Some(7000), Some(3000), Some(1000)];
        // 프레임이 한 번도 안 온 계층은 고르지 않는다.
        assert_eq!(p.available(measured), [None; 3]);
        for l in Layer::ALL {
            p.on_frame(l, t0);
        }
        // 방송자가 목표를 알리기 전에는 잰 값
        assert_eq!(p.available(measured), measured);
        p.set_rates([8000, 4000, 0]);
        assert_eq!(p.available(measured), [Some(8000), Some(4000), None]);
        // 중만 1.5초 넘게 프레임이 안 오면 뺀다. 모두 같이 멈추면(화면 멈춤) 그대로다.
        p.set_rates([8000, 4000, 2000]);
        p.on_frame(Layer::High, t0 + s(2));
        p.on_frame(Layer::Low, t0 + s(2));
        assert_eq!(p.available(measured), [Some(8000), None, Some(2000)]);
        p.on_frame(Layer::Mid, t0 + s(2));
        assert_eq!(p.available(measured), [Some(8000), Some(4000), Some(2000)]);
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
        assert_eq!(p.on_frame(Layer::Mid, true), FrameStep::Skip);
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
        assert_eq!(p.update(t0, Some(9000), &ALL), None);
        // 내림은 바로 정하고, 하 키프레임이 올 때까지 상을 그대로 보낸다.
        assert_eq!(p.update(t0, Some(3000), &ALL), Some(Layer::Low));
        assert_eq!(p.update(t0, Some(3000), &ALL), None, "한 번만 요청");
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
        assert_eq!(p.update(t0 + s(1), Some(9000), &ALL), None);
        assert_eq!(p.update(t0 + s(4), Some(9000), &ALL), None);
        // 중간에 떨어지면 다시 센다.
        assert_eq!(p.update(t0 + s(4), Some(2000), &ALL), None);
        assert_eq!(p.update(t0 + s(5), Some(9000), &ALL), None);
        assert_eq!(p.update(t0 + s(8), Some(9000), &ALL), None);
        assert_eq!(p.update(t0 + s(9), Some(9000), &ALL), Some(Layer::High));
        assert_eq!(p.keyframe_layer(), Some(Layer::High));
        // 올리던 중에 추정이 줄어 중까지만 되면 바로 중으로 낮춘다.
        assert_eq!(p.update(t0 + s(9), Some(5000), &ALL), Some(Layer::Mid));
        // 지금 계층(하)이 맞게 되면 바꾸던 것을 그만둔다.
        assert_eq!(p.update(t0 + s(10), Some(3000), &ALL), None);
        assert_eq!(p.keyframe_layer(), Some(Layer::Low));
        assert_eq!(p.on_frame(Layer::Mid, true), FrameStep::Skip);
    }

    /// D-100: 업로드가 모자라면 상부터 뺀다. 목표가 0인 상은 프레임이 최근에 왔어도 고르지 않는다.
    #[test]
    fn dropped_high_is_never_picked() {
        let t0 = Instant::now();
        let mut p = PublisherLayers::default();
        let measured = [Some(7000), Some(3000), Some(1000)];
        for l in Layer::ALL {
            p.on_frame(l, t0);
        }
        p.set_rates([0, 4000, 2000]);
        let avail = p.available(measured);
        assert_eq!(avail, [None, Some(4000), Some(2000)]);
        // 추정이 넉넉해도 상은 없으니 중이다.
        assert_eq!(best_layer(Some(50_000), &avail), Some(Layer::Mid));
        assert_eq!(best_layer(None, &avail), Some(Layer::Mid));
        // 추정이 모자라면 하, 하에도 모자라면 가장 낮은 계층(하).
        assert_eq!(best_layer(Some(3000), &avail), Some(Layer::Low));
        assert_eq!(best_layer(Some(100), &avail), Some(Layer::Low));
        // 상이 돌아온다: 목표가 다시 0이 아니고 프레임이 오면 고를 수 있다. 쉬는 동안 프레임이 1.5초 넘게 안 왔으면
        // (다른 계층은 왔다) 새 프레임이 올 때까지 못 고른다.
        p.on_frame(Layer::Mid, t0 + s(3));
        p.on_frame(Layer::Low, t0 + s(3));
        p.set_rates([6000, 4000, 2000]);
        assert_eq!(p.available(measured), [None, Some(4000), Some(2000)]);
        p.on_frame(Layer::High, t0 + s(3));
        assert_eq!(p.available(measured), [Some(6000), Some(4000), Some(2000)]);
    }

    /// 상을 보던 시청자는 상이 빠지면 바로 중으로 내려가고, 상이 돌아오면 `UP_HOLD` 뒤 올라간다.
    #[test]
    fn viewer_on_high_drops_to_mid_when_high_is_removed_and_returns_later() {
        let t0 = Instant::now();
        let mut p = LayerPicker::default();
        p.start(Some(Layer::High));
        p.on_frame(Layer::High, true);
        let no_high = [None, Some(4000), Some(2000)];
        // 추정이 넉넉해도(올림처럼 보여도) 지금 계층이 사라졌으니 기다리지 않는다.
        assert_eq!(p.update(t0, Some(50_000), &no_high), Some(Layer::Mid));
        assert_eq!(p.on_frame(Layer::High, false), FrameStep::Forward);
        assert_eq!(
            p.on_frame(Layer::Mid, true),
            FrameStep::Switched {
                from: Some(Layer::High)
            }
        );
        // 상이 돌아온다(낮춘 상 6000): 4초 동안 줄곧 올릴 수 있어야 올라간다.
        let back = [Some(6000), Some(4000), Some(2000)];
        assert_eq!(p.update(t0 + s(1), Some(50_000), &back), None);
        assert_eq!(p.update(t0 + s(4), Some(50_000), &back), None);
        assert_eq!(p.update(t0 + s(5), Some(50_000), &back), Some(Layer::High));
    }

    #[test]
    fn picker_leaves_a_dropped_layer_at_once() {
        let t0 = Instant::now();
        let mut p = LayerPicker::default();
        p.start(Some(Layer::Low));
        p.on_frame(Layer::Low, true);
        // 방송자가 하를 뺐다: 추정이 낮아도 남은 가장 낮은 계층(중)으로 바로 간다(올림이지만 기다리지 않는다).
        let no_low = [Some(8000), Some(4000), None];
        assert_eq!(p.update(t0, Some(1000), &no_low), Some(Layer::Mid));
    }
}
