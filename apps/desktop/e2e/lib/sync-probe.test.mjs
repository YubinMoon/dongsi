// sync-probe 분석 함수 시험(합성 데이터). 실행: node --test e2e/lib/sync-probe.test.mjs
import assert from "node:assert/strict";
import test from "node:test";
import { analyze, analyzeSwitch, bestClock, detectDropouts } from "./sync-probe.mjs";

// 가짜 시계: perf 1000ms ↔ QPC 100s(10^9 단위 100ns)
const QPC0 = 1_000_000_000_000; // 100000 초
const perfToQpc = (p) => QPC0 + (p - 1000) * 10000;
const clocks = [{ tag: "start", set: [{ a: 999.5, b: 1000.5, qpc: QPC0, unixMs: 1_700_000_000_000 }, { a: 990, b: 1020, qpc: QPC0 + 40000, unixMs: 1_700_000_000_000 }] }];

test("A/V = 소리 − 화면, 지연은 정초 기준", () => {
  const t0 = 1000;
  // 정초 S_k(QPC)의 perf: QPC0가 정초(10^12 = 10^5 s)이므로 perf = 1000 + 1000k ms.
  const flashes = [];
  const lb = [];
  const web = [];
  for (let k = 1; k <= 6; k++) {
    const boundaryPerf = 1000 + 1000 * k;
    flashes.push({ now: boundaryPerf + 40, edt: boundaryPerf + 60, rtp: k, mean: 235, w: 1280, h: 720 }); // 화면 지연 60ms
    lb.push(perfToQpc(boundaryPerf + 100)); // 소리 지연 100ms → A/V +40
    web.push({ outPerf: boundaryPerf + 125, now: 0, t: 0 }); // WebAudio는 25ms 더 늦음 → +65
  }
  const data = { t0Perf: t0, timeOrigin: 0, clocks, flashes, webBeeps: web, frames: [], stats: [] };
  const an = analyze(data, { beeps: lb, samples: 1, peak: 0.1 });
  assert.equal(an.counts.pairsLoopback, 6);
  assert.equal(an.counts.pairsWeb, 6);
  const seg = an.segments[0]; // 0~3s: k=1,2 (t = 1.06, 2.06)
  assert.ok(Math.abs(seg.loopback.mean - 40) < 0.01, seg.loopback.mean);
  assert.ok(Math.abs(seg.webaudio.mean - 65) < 0.01, seg.webaudio.mean);
  assert.ok(Math.abs(seg.videoDelay.mean - 60) < 0.01, seg.videoDelay.mean);
  assert.ok(Math.abs(seg.audioDelayLoopback.mean - 100) < 0.01);
  assert.equal(seg.loopback.n, 2);
});

test("EPT 차이는 비디오 − 오디오", () => {
  const stats = [{ now: 2000, wall: 0, v: { timestamp: 5000, estimatedPlayoutTimestamp: 2208988800000 + 4900 }, a: { timestamp: 5000, estimatedPlayoutTimestamp: 2208988800000 + 4950 } }];
  const an = analyze({ t0Perf: 1000, timeOrigin: 0, clocks, flashes: [], webBeeps: [], frames: [], stats }, { beeps: [] });
  assert.equal(an.series.ept[0].av, -50);
  assert.equal(an.series.ept[0].vLat, 100);
});

test("시계는 왕복이 가장 짧은 표본을 쓴다", () => {
  const c = bestClock({ clocks });
  assert.equal(c.rttMs, 1);
  assert.equal(c.qpc, QPC0);
});

function mkFrames(from, to, step, w, h) {
  const out = [];
  for (let t = from; t < to; t += step) out.push([t, t, 0, w, h, 30, out.length]);
  return out;
}

test("전환 분석: 공백·새 해상도·회복", () => {
  const clickPerf = 10_000;
  // 클릭 전 30fps 720p, 클릭 뒤 300ms 공백, 이후 1080p 30fps. 처음 500ms는 간격이 불규칙(100ms), 그 뒤 정상.
  const frames = [...mkFrames(9000, clickPerf + 50, 33.3, 1280, 720)];
  let t = clickPerf + 350;
  const irregular = [];
  while (t < clickPerf + 900) {
    irregular.push([t, t, 0, 1920, 1080, 30, 0]);
    t += 100;
  }
  frames.push(...irregular, ...mkFrames(t, clickPerf + 6000, 33.3, 1920, 1080));
  const data = { t0Perf: 0, timeOrigin: 0, clocks: [{ tag: "s", set: [{ a: 0, b: 0, qpc: 0, unixMs: 0 }] }], frames, stats: [] };
  const r = analyzeSwitch(data, { clickWall: clickPerf, stateWall: clickPerf + 80, from: { w: 1280, h: 720, codec: "h264", fps: 30 }, to: { w: 1920, h: 1080, codec: "h264", fps: 30, mime: "video/H264" } });
  assert.equal(r.stateMs, 80);
  assert.equal(Math.round(r.newMs), 350);
  assert.ok(r.maxGapMs >= 300 && r.maxGapMs < 330, r.maxGapMs);
  // 간격 100ms는 2×33.3=66.7ms 초과 → 마지막 불규칙 프레임 다음부터 정상
  assert.ok(r.recoveryMs > 800 && r.recoveryMs < 1000, r.recoveryMs);
  assert.deepEqual(r.perSec.length, 5);
});

// ---------------------------------------------------------------------------------------------
// detectDropouts: 합성 기록. 시각 축은 perf ms. t0 = 10000 이고 아래 start·k 는 t0 기준이다.
//  - 프레임은 50ms 간격(공백 {start, ms}는 start 와 start+ms 에 프레임이 있고 그 사이만 없다)
//  - 통계는 500ms 간격(k번째의 now = t0 + 500k). conceal {k, samples} = (k−1)번째와 k번째 통계 사이의 concealedSamples 증가
//  - 오디오는 500ms마다 24000 샘플이 나가므로, 구간 평균 NetEq 값 = target(k)·delay(k) ms
// ---------------------------------------------------------------------------------------------
const W = 1e12; // 가짜 벽시계(통계 wall = W + now)
function mkProbe({ t0 = 10000, secs = 12, gaps = [], conceal = [], target = () => 40, delay = () => 60, rate = null, extra = {} } = {}) {
  const frames = [];
  for (let ms = 0; ms <= secs * 1000; ms += 50) {
    if (gaps.some((g) => ms > g.start && ms < g.start + g.ms)) continue;
    frames.push([t0 + ms, t0 + ms, 0, 1280, 720, 30, frames.length]);
  }
  const stats = [];
  let cumT = 0;
  let cumD = 0;
  for (let k = 0; k <= secs * 2; k++) {
    if (k > 0) {
      cumT += (target(k) / 1000) * 24000;
      cumD += (delay(k) / 1000) * 24000;
    }
    const a = {
      concealedSamples: conceal.filter((c) => c.k <= k).reduce((s, c) => s + c.samples, 0),
      jitterBufferEmittedCount: 24000 * k,
      jitterBufferTargetDelay: cumT,
      jitterBufferDelay: cumD,
    };
    if (rate) {
      a.totalSamplesReceived = rate * 0.5 * k;
      a.totalSamplesDuration = 0.5 * k;
    }
    stats.push({ now: t0 + 500 * k, wall: W + t0 + 500 * k, v: {}, a });
  }
  return { t0Perf: t0, timeOrigin: 0, frames, stats, ...extra };
}
const near = (a, b, eps = 1e-6) => assert.ok(Math.abs(a - b) < eps, a + " ≠ " + b);

test("끊김: 비디오 공백과 오디오 은닉이 겹치면 동시 끊김(av) 하나", () => {
  const d = mkProbe({ gaps: [{ start: 4000, ms: 300 }], conceal: [{ k: 9, samples: 7440 }] });
  const r = detectDropouts(d);
  assert.deepEqual(r.counts, { av: 1, video: 0, audio: 0, total: 1 });
  const e = r.events[0];
  assert.equal(e.kind, "av");
  near(e.t, 4.0);
  near(e.videoGapMs, 300);
  near(e.audioConcealMs, 155); // 7440 / 48000 s
  // 맥락 자료가 없으면 null
  assert.equal(e.adjustMs, null);
  assert.equal(e.sinceFloorChangeMs, null);
  assert.equal(e.sinceJbtMs, null);
  assert.equal(e.cpuPct, null);
  assert.equal(e.send, null);
});

test("끊김: 비디오만 / 오디오만", () => {
  const v = detectDropouts(mkProbe({ gaps: [{ start: 6000, ms: 200 }] }));
  assert.deepEqual(v.counts, { av: 0, video: 1, audio: 0, total: 1 });
  near(v.events[0].t, 6.0);
  near(v.events[0].videoGapMs, 200);
  assert.equal(v.events[0].audioConcealMs, null);

  const a = detectDropouts(mkProbe({ conceal: [{ k: 14, samples: 4800 }] }));
  assert.deepEqual(a.counts, { av: 0, video: 0, audio: 1, total: 1 });
  near(a.events[0].audioConcealMs, 100);
  near(a.events[0].t, 6.75); // 은닉 구간 [6.5, 7.0] 의 가운데
  assert.equal(a.events[0].videoGapMs, null);
});

test("끊김: 첫 프레임 뒤 skipSec 앞은 무시", () => {
  const d = mkProbe({ gaps: [{ start: 500, ms: 300 }], conceal: [{ k: 1, samples: 7440 }] });
  assert.equal(detectDropouts(d).counts.total, 0);
  // skipSec 를 0 으로 하면 둘 다 잡히고 서로 겹쳐 av 하나
  assert.deepEqual(detectDropouts(d, { skipSec: 0 }).counts, { av: 1, video: 0, audio: 0, total: 1 });
});

test("끊김: 은닉이 문턱보다 작으면 무시, 문턱과 같으면 잡음", () => {
  assert.equal(detectDropouts(mkProbe({ conceal: [{ k: 10, samples: 480 }, { k: 12, samples: 959 }] })).counts.total, 0);
  const r = detectDropouts(mkProbe({ conceal: [{ k: 12, samples: 960 }] }));
  assert.equal(r.counts.audio, 1);
  near(r.events[0].audioConcealMs, 20);
});

test("끊김: 하나의 은닉에 비디오 공백 두 개가 겹쳐도 av 는 하나(가운데가 가까운 쪽), 나머지는 video", () => {
  const d = mkProbe({ gaps: [{ start: 3800, ms: 200 }, { start: 4300, ms: 250 }], conceal: [{ k: 9, samples: 7440 }] });
  const r = detectDropouts(d);
  assert.deepEqual(r.counts, { av: 1, video: 1, audio: 0, total: 1 + 1 });
  assert.equal(r.events[0].kind, "video"); // 3.8s 공백(은닉 가운데 4.25s 에서 0.35s 떨어짐)
  assert.equal(r.events[1].kind, "av"); // 4.3s 공백(0.175s)
  near(r.events[1].videoGapMs, 250);
});

test("끊김: 서로 멀면 av 가 아니다(은닉 구간과 공백이 ±250ms 를 넓혀도 안 겹침)", () => {
  // 은닉 [6.5, 7.0] 와 공백 [9.0, 9.2]
  const d = mkProbe({ gaps: [{ start: 9000, ms: 200 }], conceal: [{ k: 14, samples: 7440 }] });
  assert.deepEqual(detectDropouts(d).counts, { av: 0, video: 1, audio: 1, total: 2 });
});

test("끊김: 립싱크 history 로 보정값·마지막 바닥 변경 경과, jbt 경과, NetEq, CPU, 송신 쪽", () => {
  const t0 = 10000;
  const history = [
    { t: t0 + 1000, adjustMs: 0, applied: false, avOffsetMs: 3 },
    { t: t0 + 2000, adjustMs: 20, applied: true, extraField: "그대로" },
    { t: t0 + 3500, adjustMs: 25, applied: false },
    { t: t0 + 4200, adjustMs: 60, applied: true }, // 사건(4.0s) 뒤라 쓰지 않는다
  ];
  const d = mkProbe({
    t0,
    gaps: [{ start: 4000, ms: 300 }],
    conceal: [{ k: 9, samples: 7440 }],
    // 사건 끝(4.5s) 뒤 3초 = k 10..15. k=9(끝과 같은 시각)와 k=16(3초 뒤)은 제외되어야 한다.
    target: (k) => (k === 11 ? 300 : k === 12 ? 150 : k === 9 || k === 16 ? 999 : 40),
    delay: (k) => (k === 13 ? 120 : k === 9 || k === 16 ? 999 : 60),
    extra: { lipsync: { enabled: true, adjustMs: 60, floors: { videoMs: 0, audioMs: 60 }, history }, jbt: [{ t: t0 + 3000, wall: 0, v: 150, ok: true }] },
  });
  const ref = t0 + 4000;
  const publish = [
    // 사건 벽시계 = W + ref → 창은 [W+ref−1500, W+ref+500] (µs 로는 ×1000, 문자열)
    ["v", -1600, 99000], // 창 앞
    ["v", -1400, 8000],
    ["v", -1300, 32000],
    ["v", -800, 4000],
    ["v", -700, 5000],
    ["v", -100, 1000],
    ["v", 600, 77000], // 창 뒤
    ["a", -1000, 2000],
  ].map(([kind, off, lag]) => ({ kind, capture_epoch_us: String((W + ref + off) * 1000), dequeue_lag_us: String(lag) }));
  const r = detectDropouts(d, {
    cpu: [{ wall: W + ref - 300, pct: 35 }, { wall: W + ref + 700, pct: 80 }],
    publish,
  });
  assert.equal(r.counts.av, 1);
  const e = r.events[0];
  assert.equal(e.adjustMs, 25);
  assert.equal(e.sinceFloorChangeMs, 2000);
  assert.equal(e.sinceJbtMs, 1000);
  near(e.neteqTargetMs, 300);
  near(e.neteqDelayMs, 120);
  assert.equal(e.cpuPct, 35);
  assert.equal(e.send.n, 6);
  near(e.send.lagMaxMs.v, 32);
  near(e.send.lagMaxMs.a, 2);
  near(e.send.lagMaxMs.all, 32);
  near(e.send.capGapMaxMs.v, 600); // 창 안 v 캡처: −1400,−1300,−800,−700,−100 → 간격 100,500,100,600
  assert.equal(e.send.capGapMaxMs.a, null); // a 행이 하나뿐
});

test("끊김: 샘플레이트는 totalSamplesReceived/totalSamplesDuration 비율을 쓴다", () => {
  const r = detectDropouts(mkProbe({ rate: 44100, conceal: [{ k: 14, samples: 4410 }] }));
  near(r.events[0].audioConcealMs, 100);
});
