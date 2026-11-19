// sync-probe 분석 함수 시험(합성 데이터). 실행: node --test e2e/lib/sync-probe.test.mjs
import assert from "node:assert/strict";
import test from "node:test";
import { analyze, analyzeSwitch, bestClock } from "./sync-probe.mjs";

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
