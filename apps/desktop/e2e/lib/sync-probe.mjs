// 시청 페이지에서 "화면과 소리가 실제로 나온 시각"을 재는 측정 도구(앱 코드는 바꾸지 않는다).
//
// 보내는 쪽 동기 신호(crates/media/src/yuv.rs, audio/tone.rs): 테스트 패턴은 QPC 정초를 지난 첫 프레임이 전체 흰색,
// 테스트 톤은 같은 정초부터 1000Hz 100ms(진폭 0.5) 버스트다. 매 초 첫 틱이 정확히 정초라서
// 흰 프레임의 캡처 시각 = 정초 = 삐 소리의 캡처 시각이다. 시청 쪽에서 둘이 실제로 나온 시각을 같은 축에서 재면
// 비디오 지연(정초 → 화면 표시)·오디오 지연(정초 → 소리 출력)·A/V 차이를 절댓값으로 낼 수 있다.
//
// 지표 세 가지:
//   루프백  — 시청 앱 프로세스 트리의 소리를 WASAPI 프로세스 루프백으로 받아 삐 시작 시각을 QPC로 잰다(실제 출력).
//             Rust 개발용 명령 dev_tone_probe_*(apps/desktop/src-tauri/src/dev_probe.rs). 화면 시각은 rVFC
//             expectedDisplayTime(perf ms)을 dev_clock 으로 맞춘 QPC로 바꿔 비교한다.
//   WebAudio — 수신 오디오 트랙 → MediaStreamAudioSourceNode → AudioWorklet 으로 삐 시작을 찾고 getOutputTimestamp()로 perf 시각 환산.
//   EPT     — getStats 의 estimatedPlayoutTimestamp(비디오 − 오디오).
//
// A/V 부호는 앱 통계와 같다: A/V = 소리 시각 − 화면 시각 = 비디오 EPT − 오디오 EPT. 양수면 비디오가 앞선다(소리가 늦다).

import fs from "node:fs";
import path from "node:path";
import { parseCsv } from "./trace-summary.mjs";

export const NTP_UNIX_OFFSET_MS = 2208988800000;
export const SEGMENTS = [
  [0, 3],
  [3, 6],
  [6, 10],
  [10, 20],
  [20, Infinity],
];
export const segName = ([a, b]) => (b === Infinity ? a + "s~" : a + "~" + b + "s");
export const QPC_PER_MS = 10000;
export const QPC_PER_SEC = 10_000_000;

// ---------------------------------------------------------------------------------------------
// 페이지 안에서 도는 스크립트(문자열). page.evaluate("(" + PROBE_FN + ")(opts)")로 쓴다.
// 옵션: { statsMs: 500, t0Perf: 시청 시작(첫 프레임)의 performance.now(), audio: true, clockN: 17,
//         jbtStress: { periodMs, highMs } — 설치 2초 뒤부터 periodMs마다 시청 비디오 수신기 jitterBufferTarget을 highMs ↔ 0으로 바꾼다(stress 팔) }
// ---------------------------------------------------------------------------------------------
export const PROBE_FN = `async (opts) => {
  opts = opts || {};
  if (window.__sp && window.__sp.stop) window.__sp.stop();
  const video = document.querySelector("video");
  if (!video || !video.srcObject) return { error: "시청 video/srcObject가 없습니다" };
  const st = {
    alive: true,
    t0Perf: typeof opts.t0Perf === "number" ? opts.t0Perf : performance.now(),
    installPerf: performance.now(),
    installWall: Date.now(),
    timeOrigin: performance.timeOrigin,
    frames: [], flashes: [], webBeeps: [], stats: [], errors: [], clocks: [], jbt: [],
  };
  window.__sp = st;
  const r2 = (x) => Math.round(x * 100) / 100;

  // ---- 비디오: 프레임마다 16x9로 그려 평균 밝기를 본다 ----
  const cv = document.createElement("canvas");
  cv.width = 16; cv.height = 9;
  const g = cv.getContext("2d", { willReadFrequently: true });
  let prevWhite = false;
  const onFrame = (now, m) => {
    if (!st.alive) return;
    try {
      g.drawImage(video, 0, 0, 16, 9);
      const d = g.getImageData(0, 0, 16, 9).data;
      let s = 0;
      for (let i = 0; i < d.length; i += 4) s += d[i] + d[i + 1] + d[i + 2];
      const mean = s / (d.length / 4) / 3;
      const white = mean > 180;
      const edt = m.expectedDisplayTime > 0 ? m.expectedDisplayTime : now;
      st.frames.push([r2(now), r2(edt), m.rtpTimestamp === undefined ? null : m.rtpTimestamp, m.width, m.height, Math.round(mean), m.presentedFrames]);
      if (white && !prevWhite) st.flashes.push({ now: r2(now), edt: r2(edt), rtp: m.rtpTimestamp === undefined ? null : m.rtpTimestamp, mean: Math.round(mean), w: m.width, h: m.height });
      prevWhite = white;
    } catch (e) {
      if (st.errors.length < 20) st.errors.push("v:" + e);
    }
    video.requestVideoFrameCallback(onFrame);
  };
  video.requestVideoFrameCallback(onFrame);

  // ---- getStats ----
  const VF = ["timestamp", "estimatedPlayoutTimestamp", "jitterBufferDelay", "jitterBufferTargetDelay", "jitterBufferMinimumDelay", "jitterBufferEmittedCount",
    "framesDecoded", "framesReceived", "framesDropped", "freezeCount", "totalFreezesDuration", "keyFramesDecoded", "pliCount", "firCount", "nackCount",
    "packetsReceived", "packetsLost", "frameWidth", "frameHeight", "framesPerSecond", "decoderImplementation", "powerEfficientDecoder", "totalDecodeTime",
    "lastPacketReceivedTimestamp"];
  const AF = ["timestamp", "estimatedPlayoutTimestamp", "jitterBufferDelay", "jitterBufferTargetDelay", "jitterBufferMinimumDelay", "jitterBufferEmittedCount",
    "packetsReceived", "packetsLost", "totalSamplesReceived", "concealedSamples", "insertedSamplesForDeceleration", "removedSamplesForAcceleration", "audioLevel",
    "totalSamplesDuration", "concealmentEvents", "silentConcealedSamples", "lastPacketReceivedTimestamp"];
  // media-playout(RTCAudioPlayoutStats)의 숫자 필드 전부(totalPlayoutDelay, totalSamplesCount, synthesizedSamplesDuration 등). 없으면 null
  const numericFields = (o) => { if (!o) return null; const r = {}; for (const k in o) if (typeof o[k] === "number") r[k] = o[k]; return r; };
  const pick = (o, ks) => { if (!o) return null; const r = {}; for (const k of ks) if (o[k] !== undefined) r[k] = o[k]; return r; };
  let statsBusy = false;
  const readStats = async () => {
    if (statsBusy || !st.alive) return;
    statsBusy = true;
    try {
      const pcs = (window.__t4pcs || []).filter((pc) => pc.connectionState !== "closed");
      const pc = pcs[pcs.length - 1];
      if (!pc) return;
      const now = performance.now();
      const rep = await pc.getStats();
      let v = null, a = null, mp = null;
      const byId = {};
      rep.forEach((s) => { byId[s.id] = s; if (s.type === "inbound-rtp" && s.kind === "video") v = s; if (s.type === "inbound-rtp" && s.kind === "audio") a = s; if (s.type === "media-playout" && (!mp || s.kind === "audio")) mp = s; });
      st.stats.push({ now: r2(now), wall: Date.now(), v: pick(v, VF), a: pick(a, AF), p: numericFields(mp),
        vMime: v && byId[v.codecId] ? byId[v.codecId].mimeType : null, aMime: a && byId[a.codecId] ? byId[a.codecId].mimeType : null });
    } catch (e) {
      if (st.errors.length < 20) st.errors.push("s:" + e);
    } finally {
      statsBusy = false;
    }
  };
  const timer = setInterval(readStats, opts.statsMs || 500);
  readStats();

  // ---- 시계 맞추기: perf ms ↔ QPC. 왕복이 가장 짧은 표본을 쓴다(분석 쪽에서 고른다) ----
  const syncClock = async (tag, n) => {
    const set = [];
    for (let i = 0; i < n; i++) {
      const a = performance.now();
      let r;
      try { r = await window.__t4invoke("dev_clock"); } catch (e) { st.errors.push("clock:" + (e && e.message ? e.message : JSON.stringify(e))); break; }
      const b = performance.now();
      set.push({ a, b, qpc: r.qpc100ns, unixMs: r.unixMs });
      await new Promise((res) => setTimeout(res, 12));
    }
    st.clocks.push({ tag, set });
    return set.length;
  };
  st.syncClock = syncClock;

  // ---- WebAudio: 삐 시작 찾기 ----
  const setupAudio = async () => {
    const track = video.srcObject.getAudioTracks()[0];
    if (!track || opts.audio === false) { st.errors.push("a:오디오 트랙 없음"); return; }
    try {
      const ac = new AudioContext({ latencyHint: "interactive" });
      st.ac = ac;
      const code = "class P extends AudioWorkletProcessor{constructor(){super();this.q=1e9;this.need=Math.round(0.05*sampleRate);}" +
        "process(i){const c=i[0]&&i[0][0];if(!c)return true;for(let k=0;k<c.length;k++){const v=Math.abs(c[k]);" +
        "if(v>0.08){if(this.q>=this.need)this.port.postMessage({t:currentTime+k/sampleRate});this.q=0;}else this.q++;}return true;}}" +
        "registerProcessor('sp-beep',P);";
      await ac.audioWorklet.addModule(URL.createObjectURL(new Blob([code], { type: "application/javascript" })));
      const src = ac.createMediaStreamSource(new MediaStream([track]));
      const node = new AudioWorkletNode(ac, "sp-beep");
      const mute = ac.createGain();
      mute.gain.value = 0; // 앱의 video 가 이미 소리를 내므로 두 번 들리지 않게 막는다
      src.connect(node); node.connect(mute); mute.connect(ac.destination);
      node.port.onmessage = (e) => {
        if (!st.alive) return;
        const ts = ac.getOutputTimestamp();
        st.webBeeps.push({ now: r2(performance.now()), t: e.data.t, outPerf: r2(ts.performanceTime + (e.data.t - ts.contextTime) * 1000),
          tsCtx: ts.contextTime, tsPerf: r2(ts.performanceTime), outLat: ac.outputLatency, baseLat: ac.baseLatency });
      };
      if (ac.state !== "running") await ac.resume().catch(() => {});
      st.acState = ac.state;
      st.acSampleRate = ac.sampleRate;
    } catch (e) {
      st.errors.push("a:" + e);
    }
  };
  await Promise.all([setupAudio(), syncClock("start", opts.clockN || 17)]);

  // ---- stress 팔: 시청 PC의 비디오 수신기 jitterBufferTarget 을 highMs ↔ 0 으로 번갈아 바꾼다(변경 자체가 끊김을 부르는지 시험) ----
  let stressStart = null, stressTimer = null;
  if (opts.jbtStress && opts.jbtStress.periodMs > 0) {
    let high = true;
    const tick = () => {
      if (!st.alive) return;
      const value = high ? opts.jbtStress.highMs : 0;
      const e = { t: r2(performance.now()), wall: Date.now(), v: value, ok: false };
      try {
        const pcs = (window.__t4pcs || []).filter((pc) => pc.connectionState !== "closed");
        const pc = pcs[pcs.length - 1];
        const recv = pc ? pc.getReceivers().filter((r) => r.track && r.track.kind === "video") : [];
        for (const r of recv) { r.jitterBufferTarget = value; e.ok = true; }
      } catch (err) {
        e.err = String(err);
      }
      st.jbt.push(e);
      high = !high;
    };
    stressStart = setTimeout(() => {
      tick();
      stressTimer = setInterval(tick, opts.jbtStress.periodMs);
    }, Math.max(0, 2000 - (performance.now() - st.installPerf)));
  }
  st.stop = () => { st.alive = false; clearInterval(timer); clearTimeout(stressStart); clearInterval(stressTimer); try { st.ac && st.ac.close(); } catch (e) {} };
  return { ok: true, acState: st.acState || null, errors: st.errors, installMs: Math.round(performance.now() - st.installPerf) };
}`;

/** 기록을 JSON으로 가져온다. 끝 시계도 한 번 더 맞춘다(표류 확인). */
export const FINISH_FN = `async () => {
  const s = window.__sp;
  if (!s) return null;
  if (s.syncClock) await s.syncClock("end", 17);
  const video = document.querySelector("video");
  return {
    jbt: s.jbt || [],
    t0Perf: s.t0Perf, installPerf: s.installPerf, installWall: s.installWall, timeOrigin: s.timeOrigin,
    frames: s.frames, flashes: s.flashes, webBeeps: s.webBeeps, stats: s.stats, errors: s.errors, clocks: s.clocks,
    acState: s.ac ? s.ac.state : null, acSampleRate: s.acSampleRate || null,
    video: video ? { muted: video.muted, volume: video.volume, paused: video.paused, w: video.videoWidth, h: video.videoHeight } : null,
  };
}`;

export const STOP_FN = `() => { if (window.__sp && window.__sp.stop) window.__sp.stop(); return true; }`;

// ---------------------------------------------------------------------------------------------
// Node 쪽 도우미
// ---------------------------------------------------------------------------------------------

/** 페이지에 측정 스크립트를 넣는다. 반환: 설치 결과(오류 포함). */
export async function installProbe(page, opts = {}) {
  return page.evaluate("(" + PROBE_FN + ")(" + JSON.stringify(opts) + ")");
}

/** 기록을 가져온다(끝 시계 맞춤 포함). */
export async function finishProbe(page) {
  return page.evaluate("(" + FINISH_FN + ")()");
}

export async function stopProbe(page) {
  await page.evaluate("(" + STOP_FN + ")()").catch(() => {});
}

/** 시청 앱의 프로세스 트리 루프백 측정을 켠다. 실패해도 던지지 않고 { error }를 돌려준다. */
export async function startLoopback(app, opts = {}) {
  const r = await app.invoke("dev_tone_probe_start", { pid: opts.pid ?? null, device: opts.device ?? null }, { raw: true });
  return r.ok ? r.v : { error: r.e?.message ?? JSON.stringify(r.e) };
}

/** 지금까지의 삐(QPC 100ns)와 통계를 가져온다. 실패하면 { error }. */
export async function takeLoopback(app) {
  const r = await app.invoke("dev_tone_probe_take", undefined, { raw: true });
  return r.ok ? r.v : { error: r.e?.message ?? JSON.stringify(r.e), beeps: [], samples: 0, peak: 0 };
}

export async function stopLoopback(app) {
  await app.invoke("dev_tone_probe_stop", undefined, { raw: true }).catch(() => {});
}

// ---------------------------------------------------------------------------------------------
// 분석(Node, 순수 함수)
// ---------------------------------------------------------------------------------------------

const mean = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);
const maxAbs = (xs) => (xs.length ? Math.max(...xs.map(Math.abs)) : null);

/** 모든 시계 표본 중 왕복이 가장 짧은 것을 쓴다. 시작·끝 표본의 차이로 표류도 낸다. */
export function bestClock(data) {
  let best = null;
  const perTag = {};
  for (const c of data.clocks ?? []) {
    for (const s of c.set ?? []) {
      const rtt = s.b - s.a;
      const cand = { rtt, perf: (s.a + s.b) / 2, qpc: s.qpc, unixMs: s.unixMs, tag: c.tag };
      if (!best || rtt < best.rtt) best = cand;
      if (!perTag[c.tag] || rtt < perTag[c.tag].rtt) perTag[c.tag] = cand;
    }
  }
  if (!best) return null;
  // 표류: 각 묶음의 최선 표본이 같은 변환을 주는지(perf → QPC 차이, ms)
  let driftMs = null;
  const tags = Object.keys(perTag);
  if (tags.length >= 2) {
    const a = perTag[tags[0]];
    const b = perTag[tags[tags.length - 1]];
    const qpcFromA = a.qpc + (b.perf - a.perf) * QPC_PER_MS;
    driftMs = (b.qpc - qpcFromA) / QPC_PER_MS;
  }
  return { ...best, driftMs, rttMs: best.rtt };
}

/** 값 목록 [{t, v}]에서 구간별 평균·최대|값|·개수. */
export function segSummary(items, segments = SEGMENTS) {
  return segments.map((seg) => {
    const vs = items.filter((x) => x.t >= seg[0] && x.t < seg[1]).map((x) => x.v);
    return { seg: segName(seg), n: vs.length, mean: mean(vs), maxAbs: maxAbs(vs) };
  });
}

/**
 * 한 번의 시청 기록을 분석한다.
 * @param data finishProbe 결과
 * @param loopback takeLoopback 결과({ beeps: [qpc...], samples, peak })
 * 반환: { clock, counts, series, segments, summary }
 *   series.loopback / webaudio: [{ t(초, 시청 시작 기준), av(ms), vDelay(ms), aDelay(ms) }]
 *   series.ept: [{ t, av(ms), vLat(ms) }]
 *   series.video: [{ t, v: 비디오 지연(ms) }]  (흰 프레임마다)
 */
export function analyze(data, loopback, { segments = SEGMENTS, pairWindowMs = 450 } = {}) {
  const clk = bestClock(data);
  if (!clk) return { error: "시계 표본이 없습니다(dev_clock 실패)", clock: null };
  const toQpc = (perf) => clk.qpc + (perf - clk.perf) * QPC_PER_MS;
  const toPerf = (qpc) => clk.perf + (qpc - clk.qpc) / QPC_PER_MS;
  const t0 = data.t0Perf;
  const flashes = (data.flashes ?? []).map((f) => {
    const q = toQpc(f.edt);
    const boundary = Math.floor(q / QPC_PER_SEC) * QPC_PER_SEC; // 흰 프레임의 캡처 시각(정초)
    return { perf: f.edt, qpc: q, boundary, t: (f.edt - t0) / 1000, vDelay: (q - boundary) / QPC_PER_MS, w: f.w, h: f.h };
  });
  const pairUp = (beepPerfs, beepQpcs) => {
    const out = [];
    const used = new Set();
    for (const f of flashes) {
      let bi = -1;
      let bd = Infinity;
      for (let i = 0; i < beepPerfs.length; i++) {
        if (used.has(i)) continue;
        const d = Math.abs(beepPerfs[i] - f.perf);
        if (d < bd) {
          bd = d;
          bi = i;
        }
      }
      if (bi < 0 || bd > pairWindowMs) continue;
      used.add(bi);
      out.push({ t: f.t, av: beepPerfs[bi] - f.perf, vDelay: f.vDelay, aDelay: (beepQpcs[bi] - f.boundary) / QPC_PER_MS });
    }
    return out;
  };
  const lbQpc = (loopback?.beeps ?? []).slice();
  const lbPerf = lbQpc.map(toPerf);
  const wa = data.webBeeps ?? [];
  const waPerf = wa.map((b) => b.outPerf);
  const waQpc = waPerf.map(toQpc);
  const series = {
    video: flashes.map((f) => ({ t: f.t, v: f.vDelay })),
    loopback: pairUp(lbPerf, lbQpc),
    webaudio: pairUp(waPerf, waQpc),
    ept: [],
  };
  for (const s of data.stats ?? []) {
    const v = s.v;
    const a = s.a;
    if (!v || !a || !(v.estimatedPlayoutTimestamp > 0) || !(a.estimatedPlayoutTimestamp > 0)) continue;
    series.ept.push({
      t: (s.now - t0) / 1000,
      av: v.estimatedPlayoutTimestamp - a.estimatedPlayoutTimestamp,
      vLat: (v.timestamp ?? s.wall) - (v.estimatedPlayoutTimestamp - NTP_UNIX_OFFSET_MS),
      aLat: (a.timestamp ?? s.wall) - (a.estimatedPlayoutTimestamp - NTP_UNIX_OFFSET_MS),
    });
  }
  const avSeg = (arr) => segSummary(arr.map((x) => ({ t: x.t, v: x.av })), segments);
  const field = (arr, k) => segSummary(arr.map((x) => ({ t: x.t, v: x[k] })), segments);
  const segs = segments.map((seg, i) => ({
    seg: segName(seg),
    loopback: avSeg(series.loopback)[i],
    webaudio: avSeg(series.webaudio)[i],
    ept: avSeg(series.ept)[i],
    videoDelay: field(series.video.map((x) => ({ t: x.t, av: x.v })), "av")[i],
    audioDelayLoopback: field(series.loopback, "aDelay")[i],
    audioDelayWeb: field(series.webaudio, "aDelay")[i],
    eptVideoLatency: field(series.ept, "vLat")[i],
    eptAudioLatency: field(series.ept, "aLat")[i],
  }));
  const firstFlashT = flashes.length ? flashes[0].t : null;
  return {
    clock: { rttMs: clk.rttMs, driftMs: clk.driftMs, tag: clk.tag },
    counts: {
      frames: (data.frames ?? []).length,
      flashes: flashes.length,
      loopbackBeeps: lbQpc.length,
      webBeeps: wa.length,
      pairsLoopback: series.loopback.length,
      pairsWeb: series.webaudio.length,
      ept: series.ept.length,
      loopbackSamples: loopback?.samples ?? null,
      loopbackPeak: loopback?.peak ?? null,
      loopbackError: loopback?.error ?? loopback?.lastError ?? null,
    },
    firstFlashT,
    series,
    segments: segs,
    flashes,
  };
}

/** 구간별 값(루프백 우선, 없으면 WebAudio, 없으면 EPT)의 [from초, ∞) 최대|값|. 검사용. */
export function primaryAv(an) {
  for (const key of ["loopback", "webaudio", "ept"]) {
    if (an.series[key].length) return { key, series: an.series[key] };
  }
  return { key: null, series: [] };
}

// ---------------------------------------------------------------------------------------------
// 전환(화질 변경) 분석
// ---------------------------------------------------------------------------------------------

/** wall(Date.now) ms → 페이지 perf ms. dev_clock 표본의 unixMs를 쓴다. */
export function wallToPerf(data, wall) {
  const clk = bestClock(data);
  if (clk && typeof clk.unixMs === "number") return clk.perf + (wall - clk.unixMs);
  return wall - data.timeOrigin;
}

/**
 * 전환 하나를 분석한다.
 * @param ev { clickWall, stateWall, from: {label, w, h, fps}, to: {...}, windowSec }
 * 새 화면은 해상도가 바뀔 때만 찾는다(주사율만 바뀌면 newMs = null).
 * 회복 = 클릭 뒤(새 화면이 처음 나온 뒤) 표시 프레임 간격이 처음으로 1초 동안 계속 2×프레임 간격 이하가 된 시점(그 1초 구간의 시작).
 */
export function analyzeSwitch(data, ev, { windowSec = 5, nextClickPerf = null } = {}) {
  const c = wallToPerf(data, ev.clickWall);
  const frames = (data.frames ?? []).map((f) => ({ now: f[0], edt: f[1], w: f[3], h: f[4] })).sort((a, b) => a.edt - b.edt);
  const stats = data.stats ?? [];
  const end = c + windowSec * 1000;
  const T = 1000 / (ev.to.fps || 30);
  const limit = 2 * T + 1;
  const resChanged = ev.from.w !== ev.to.w || ev.from.h !== ev.to.h;

  // 새 구성이 처음 화면에 나온 시각
  let firstNewPerf = null;
  let firstNewBy = null;
  if (resChanged) {
    const f = frames.find((x) => x.edt >= c && x.w === ev.to.w && x.h === ev.to.h);
    if (f) {
      firstNewPerf = f.edt;
      firstNewBy = "resolution";
    }
  }
  const newMs = firstNewPerf === null ? null : firstNewPerf - c;

  // 최대 화면 공백: 클릭 직전 프레임 ~ 클릭 뒤 windowSec 안의 연속 프레임 간격
  let maxGap = null;
  for (let i = 1; i < frames.length; i++) {
    if (frames[i].edt < c || frames[i].edt > end) continue;
    const gap = frames[i].edt - frames[i - 1].edt;
    if (maxGap === null || gap > maxGap) maxGap = gap;
  }
  // 윈도 끝까지 프레임이 더 안 나온 경우도 공백으로 본다
  const lastIn = frames.filter((f) => f.edt <= end).at(-1);
  const lastAll = frames.at(-1);
  if (lastIn && lastAll && lastAll.edt <= end && (!nextClickPerf || nextClickPerf >= end)) {
    const tail = (data.stats?.at(-1)?.now ?? lastAll.edt) - lastAll.edt;
    if (maxGap === null || tail > maxGap) maxGap = tail;
  }

  const perSec = [];
  for (let i = 0; i < windowSec; i++) {
    perSec.push(frames.filter((f) => f.edt >= c + i * 1000 && f.edt < c + (i + 1) * 1000).length);
  }

  const at = (perf) => {
    let r = null;
    for (const s of stats) if (s.now <= perf) r = s;
    return r;
  };
  const b = at(c);
  const a = at(end) ?? stats.at(-1);
  const delta = (k) => (b?.v && a?.v && typeof a.v[k] === "number" && typeof b.v[k] === "number" ? a.v[k] - b.v[k] : null);
  const deltas = {
    keyFrames: delta("keyFramesDecoded"),
    pli: delta("pliCount"),
    freeze: delta("freezeCount"),
    lost: delta("packetsLost"),
    nack: delta("nackCount"),
    framesDecoded: delta("framesDecoded"),
  };

  // 회복
  const startFrom = Math.max(c, firstNewPerf ?? c);
  const limitEnd = nextClickPerf ?? Infinity;
  let recovery = null;
  for (let i = 0; i < frames.length; i++) {
    const s0 = frames[i].edt;
    if (s0 < startFrom || s0 >= limitEnd) continue;
    let ok = true;
    let covered = false;
    for (let j = i; j + 1 < frames.length; j++) {
      if (frames[j].edt > s0 + 1000) break;
      if (frames[j + 1].edt - frames[j].edt > limit) {
        ok = false;
        break;
      }
      if (frames[j + 1].edt >= s0 + 1000) covered = true;
    }
    if (ok && covered) {
      recovery = s0 - c;
      break;
    }
  }
  return {
    clickPerf: c,
    stateMs: typeof ev.stateWall === "number" ? ev.stateWall - ev.clickWall : null,
    newMs,
    newBy: firstNewBy,
    maxGapMs: maxGap,
    perSec,
    deltas,
    recoveryMs: recovery,
    frameIntervalMs: T,
  };
}


// ---------------------------------------------------------------------------------------------
// 끊김(오디오·비디오가 함께 끊기는 현상) 검출
// ---------------------------------------------------------------------------------------------

/** DONGSI_MEDIA_TRACE 폴더의 publish.csv 행(없으면 null). 열은 crates/net/src/publish.rs 머리 문서. */
export function readPublishRows(dir) {
  const p = path.join(dir, "publish.csv");
  if (!fs.existsSync(p)) return null;
  return parseCsv(fs.readFileSync(p, "utf8")).rows;
}

/** 통계 한 개의 오디오 샘플레이트(Hz): totalSamplesReceived ÷ totalSamplesDuration, 없거나 이상하면 48000. */
function audioRate(a) {
  const r = a && a.totalSamplesReceived > 0 && a.totalSamplesDuration > 0 ? a.totalSamplesReceived / a.totalSamplesDuration : 48000;
  return r >= 8000 && r <= 96000 ? r : 48000;
}

/**
 * 한 번의 시청 기록에서 화면·소리 끊김을 센다(순수 함수). 시각 축은 모두 페이지 performance.now(ms)
 * (frames, stats[].now, jbt[].t가 같은 축). 결과의 t는 첫 프레임(data.t0Perf) 기준 초.
 *
 * 검출:
 *  - 비디오 공백: rVFC frames 의 expectedDisplayTime 간격 ≥ gapMs(기본 150). 공백 시작(앞 프레임)이 첫 프레임 뒤 skipSec(기본 1.0)초 이후인 것.
 *  - 오디오 은닉: 연속한 두 통계 사이 concealedSamples 증가 ≥ concealMinSamples(기본 960 = 48kHz 20ms). 구간 가운데가 skipSec 이후인 것.
 *    ms 환산은 totalSamplesReceived/totalSamplesDuration(없으면 48000Hz).
 * 분류: 비디오 공백 [시작, 끝]과 은닉 통계 구간 [이전 now, 이번 now]을 각각 ±overlapMs(기본 250) 넓혀 겹치면 av(동시 끊김),
 *  아니면 video / audio. 은닉 하나는 한 번만 짝짓고, 겹치는 공백이 여럿이면 가운데가 가장 가까운 것 하나만 av, 나머지는 video.
 * 사건의 기준 시각 ref: video·av = 공백 시작, audio = 은닉 구간의 가운데. 사건 끝 end: 공백 끝과 은닉 구간 끝 중 늦은 것.
 *
 * 사건마다 붙이는 맥락(없으면 null):
 *  sinceJbtMs                      마지막 jbt 변경(stress 팔) 뒤 지난 ms
 *  neteqTargetMs / neteqDelayMs    사건 끝 뒤 afterMs(기본 3000) 안, 통계 구간 평균(Δ jitterBufferTargetDelay·jitterBufferDelay ÷ Δ jitterBufferEmittedCount)의 최댓값
 *  cpuPct                          ref 시각에 가장 가까운 CPU 표본(opts.cpu = [{wall, pct}], wall 은 Date.now)
 *  send                            opts.publish(publish.csv 행)가 있을 때: ref 의 벽시계 [−1500ms, +500ms] 안 캡처 행의
 *                                  { n, lagMaxMs: {v, a, all}, capGapMaxMs: {v, a} }  (큐 지연 dequeue_lag_us 최댓값, 종류별 캡처 간격 최댓값)
 */
export function detectDropouts(data, opts = {}) {
  const o = { gapMs: 150, skipSec: 1.0, concealMinSamples: 960, overlapMs: 250, afterMs: 3000, ...Object.fromEntries(Object.entries(opts).filter(([, v]) => v !== undefined)) };
  const t0 = data.t0Perf ?? 0;
  const skipPerf = t0 + o.skipSec * 1000;
  const stats = data.stats ?? [];

  // 비디오 공백
  const edts = (data.frames ?? []).map((f) => f[1]).filter(Number.isFinite).sort((a, b) => a - b);
  const gaps = [];
  for (let i = 1; i < edts.length; i++) {
    const ms = edts[i] - edts[i - 1];
    if (ms >= o.gapMs && edts[i - 1] >= skipPerf) gaps.push({ start: edts[i - 1], end: edts[i], ms, used: false });
  }
  // 오디오 은닉
  const conceals = [];
  for (let i = 1; i < stats.length; i++) {
    const cur = stats[i].a;
    const prev = stats[i - 1].a;
    if (!cur || !prev || !Number.isFinite(cur.concealedSamples) || !Number.isFinite(prev.concealedSamples)) continue;
    const samples = cur.concealedSamples - prev.concealedSamples;
    if (samples < o.concealMinSamples) continue;
    const start = stats[i - 1].now;
    const end = stats[i].now;
    if ((start + end) / 2 < skipPerf) continue;
    conceals.push({ start, end, samples, ms: (samples / audioRate(cur)) * 1000 });
  }

  // 분류
  const ov = o.overlapMs;
  const events = [];
  for (const c of conceals) {
    let best = null;
    for (const g of gaps) {
      if (g.used) continue;
      if (!(g.start - ov < c.end + ov && c.start - ov < g.end + ov)) continue;
      const dist = Math.abs((g.start + g.end) / 2 - (c.start + c.end) / 2);
      if (!best || dist < best.dist) best = { g, dist };
    }
    if (best) {
      best.g.used = true;
      events.push({ kind: "av", ref: best.g.start, end: Math.max(best.g.end, c.end), videoGapMs: best.g.ms, audioConcealMs: c.ms, concealedSamples: c.samples });
    } else {
      events.push({ kind: "audio", ref: (c.start + c.end) / 2, end: c.end, videoGapMs: null, audioConcealMs: c.ms, concealedSamples: c.samples });
    }
  }
  for (const g of gaps) if (!g.used) events.push({ kind: "video", ref: g.start, end: g.end, videoGapMs: g.ms, audioConcealMs: null, concealedSamples: null });
  events.sort((a, b) => a.ref - b.ref);

  // 맥락
  const jbt = (data.jbt ?? []).filter((j) => Number.isFinite(j.t)).sort((a, b) => a.t - b.t);
  const last = (arr, pred) => {
    let r = null;
    for (const x of arr) if (pred(x)) r = x;
    return r;
  };
  const perfToWall = (perf) => {
    let best = null;
    for (const s of stats) if (Number.isFinite(s.wall) && (!best || Math.abs(s.now - perf) < Math.abs(best.now - perf))) best = s;
    if (best) return best.wall + (perf - best.now);
    return Number.isFinite(data.timeOrigin) ? data.timeOrigin + perf : null;
  };
  const neteq = (end) => {
    let target = null;
    let delay = null;
    for (let i = 1; i < stats.length; i++) {
      const cur = stats[i].a;
      const prev = stats[i - 1].a;
      if (stats[i].now <= end || stats[i].now > end + o.afterMs || !cur || !prev) continue;
      const de = cur.jitterBufferEmittedCount - prev.jitterBufferEmittedCount;
      if (!(de > 0)) continue;
      const t = ((cur.jitterBufferTargetDelay - prev.jitterBufferTargetDelay) / de) * 1000;
      const d = ((cur.jitterBufferDelay - prev.jitterBufferDelay) / de) * 1000;
      if (Number.isFinite(t) && (target === null || t > target)) target = t;
      if (Number.isFinite(d) && (delay === null || d > delay)) delay = d;
    }
    return { target, delay };
  };
  const cpuAt = (wall) => {
    if (wall === null || !opts.cpu?.length) return null;
    let best = null;
    for (const c of opts.cpu) if (!best || Math.abs(c.wall - wall) < Math.abs(best.wall - wall)) best = c;
    return best ? best.pct : null;
  };
  const sendAt = (wall) => {
    if (!opts.publish || wall === null) return null;
    const lo = (wall - 1500) * 1000;
    const hi = (wall + 500) * 1000;
    const rows = opts.publish
      .map((r) => ({ kind: r.kind, cap: Number(r.capture_epoch_us), lag: Number(r.dequeue_lag_us) }))
      .filter((r) => Number.isFinite(r.cap) && r.cap >= lo && r.cap <= hi);
    const lagMax = (kind) => {
      const xs = rows.filter((r) => (kind ? r.kind === kind : true) && Number.isFinite(r.lag)).map((r) => r.lag / 1000);
      return xs.length ? Math.max(...xs) : null;
    };
    const gapMax = (kind) => {
      const caps = rows.filter((r) => r.kind === kind).map((r) => r.cap).sort((a, b) => a - b);
      let m = null;
      for (let i = 1; i < caps.length; i++) {
        const g = (caps[i] - caps[i - 1]) / 1000;
        if (m === null || g > m) m = g;
      }
      return m;
    };
    return { n: rows.length, lagMaxMs: { v: lagMax("v"), a: lagMax("a"), all: lagMax(null) }, capGapMaxMs: { v: gapMax("v"), a: gapMax("a") } };
  };
  for (const e of events) {
    e.t = (e.ref - t0) / 1000;
    const j = last(jbt, (x) => x.t <= e.ref);
    e.sinceJbtMs = j ? e.ref - j.t : null;
    const n = neteq(e.end);
    e.neteqTargetMs = n.target;
    e.neteqDelayMs = n.delay;
    const wall = perfToWall(e.ref);
    e.cpuPct = cpuAt(wall);
    e.send = sendAt(wall);
  }
  const count = (k) => events.filter((e) => e.kind === k).length;
  return {
    params: { gapMs: o.gapMs, skipSec: o.skipSec, concealMinSamples: o.concealMinSamples, overlapMs: o.overlapMs, afterMs: o.afterMs },
    counts: { av: count("av"), video: count("video"), audio: count("audio"), total: events.length },
    events,
  };
}

// ---------------------------------------------------------------------------------------------
// 표시용
// ---------------------------------------------------------------------------------------------
export const fmt = (x, d = 0, sign = false) => {
  if (x === null || x === undefined || Number.isNaN(x)) return "–";
  const s = Number(x).toFixed(d);
  return sign && x > 0 ? "+" + s : s;
};
