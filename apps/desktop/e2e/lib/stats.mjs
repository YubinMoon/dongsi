// 시청 지표 측정: 시청 화면의 RTCPeerConnection.getStats()를 1초마다 읽어 합친다.
import { STATS_FN } from "./page-scripts.mjs";
import { sleep } from "./util.mjs";

/** NTP(1900) → 유닉스(1970) 밀리초 차이(앱의 watchStats.ts와 같다) */
const NTP_UNIX_OFFSET_MS = 2208988800000;

export const readStats = (page) => page.evaluate(`(${STATS_FN})()`);

const mean = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);

function jitterMs(first, last) {
  if (!first || !last) return null;
  const dc = (last.jitterBufferEmittedCount ?? 0) - (first.jitterBufferEmittedCount ?? 0);
  if (!(dc > 0)) return null;
  return (((last.jitterBufferDelay ?? 0) - (first.jitterBufferDelay ?? 0)) / dc) * 1000;
}

/**
 * seconds초 동안 1초마다 통계를 읽어 요약한다.
 * 반환: { samples, ok, summary } — summary는 검사에 쓸 값(프레임 증가, 해상도, 오디오 패킷 증가, A/V, 지연 ...).
 */
export async function measureWatch(page, { seconds = 10 } = {}) {
  const samples = [];
  const start = Date.now();
  for (let i = 0; i <= seconds; i++) {
    const t = Date.now();
    const s = await readStats(page);
    if (s) samples.push({ ...s, t: t - start });
    const wait = start + (i + 1) * 1000 - Date.now();
    if (i < seconds && wait > 0) await sleep(wait);
  }
  const good = samples.filter((s) => s.video && s.audio);
  if (good.length < 2) {
    return { samples, summary: null, error: "통계를 읽지 못했습니다(시청 PeerConnection이 없거나 트랙이 없음): " + JSON.stringify(samples.at(-1) ?? null) };
  }
  const first = good[0];
  const last = good.at(-1);
  // 3초 뒤 샘플(없으면 가장 가까운 것)
  const at3 = good.find((s) => s.t >= first.t + 2900) ?? last;
  const lat = [];
  const av = [];
  for (const s of good) {
    const vp = s.video.estimatedPlayoutTimestamp;
    const ap = s.audio.estimatedPlayoutTimestamp;
    if (typeof vp === "number" && vp > 0) {
      lat.push((s.video.timestamp ?? s.nowMs) - (vp - NTP_UNIX_OFFSET_MS));
      if (typeof ap === "number" && ap > 0) av.push(vp - ap);
    }
  }
  const summary = {
    width: last.video.frameWidth ?? 0,
    height: last.video.frameHeight ?? 0,
    codec: last.videoMime ? last.videoMime.replace(/^video\//, "") : null,
    audioCodec: last.audioMime ? last.audioMime.replace(/^audio\//, "") : null,
    framesIn3s: (at3.video.framesDecoded ?? 0) - (first.video.framesDecoded ?? 0),
    framesTotal: (last.video.framesDecoded ?? 0) - (first.video.framesDecoded ?? 0),
    framesDropped: last.video.framesDropped ?? 0,
    freezeCount: last.video.freezeCount ?? 0,
    fps: last.video.framesPerSecond ?? null,
    audioPacketsDelta: (last.audio.packetsReceived ?? 0) - (first.audio.packetsReceived ?? 0),
    audioSamplesDelta: (last.audio.totalSamplesReceived ?? 0) - (first.audio.totalSamplesReceived ?? 0),
    audioEnergy: last.audio.totalAudioEnergy ?? 0,
    audioLevelMax: Math.max(...good.map((s) => s.audio.audioLevel ?? 0)),
    jbVideoMs: jitterMs(first.video, last.video),
    jbAudioMs: jitterMs(first.audio, last.audio),
    avMean: mean(av),
    avMaxAbs: av.length ? Math.max(...av.map(Math.abs)) : null,
    avCount: av.length,
    latMean: mean(lat),
    latMax: lat.length ? Math.max(...lat) : null,
    latCount: lat.length,
    rttMs: last.rttMs,
    decoder: last.video.decoderImplementation ?? null,
    pcCount: last.pcCount,
    connectionState: last.connectionState,
    durationSec: Math.round((last.t - first.t) / 100) / 10,
  };
  return { samples, summary };
}

/**
 * 시청 지표를 검사 항목으로 기록한다.
 * expect: { width, height }(방송자 쪽 값). 기준: 3초에 프레임 30 이상, 해상도 일치, 오디오 패킷 증가, 테스트 톤 에너지 > 0.
 * A/V 차이·지연은 기록이 목적: |A/V 평균| > 100ms 또는 지연 평균 > 500ms면 경고.
 */
export function recordWatch(rec, label, m, expect = {}) {
  if (!m.summary) {
    rec.check(label + ": 통계 읽기", false, null, m.error);
    return null;
  }
  const s = m.summary;
  rec.check(label + ": 영상 프레임이 3초에 30 이상 늘어남", s.framesIn3s >= 30, s.framesIn3s + "개(3초), 총 " + s.framesTotal + "개/" + s.durationSec + "초");
  if (expect.width && expect.height) {
    rec.check(label + ": 해상도가 방송자와 같음", s.width === expect.width && s.height === expect.height, s.width + "×" + s.height, "방송자 " + expect.width + "×" + expect.height);
  } else {
    rec.info(label + ": 해상도", s.width + "×" + s.height);
  }
  rec.check(label + ": 오디오 패킷이 늘어남", s.audioPacketsDelta > 0, "패킷 +" + s.audioPacketsDelta + ", 샘플 +" + s.audioSamplesDelta);
  rec.check(label + ": 테스트 톤이 들림(totalAudioEnergy > 0)", s.audioEnergy > 0, "에너지 " + s.audioEnergy.toFixed(5) + ", 최대 audioLevel " + s.audioLevelMax.toFixed(4));
  rec.info(label + ": 코덱·디코더·지터버퍼", { codec: s.codec, audio: s.audioCodec, decoder: s.decoder, jbVideoMs: round(s.jbVideoMs), jbAudioMs: round(s.jbAudioMs), framesDropped: s.framesDropped, freeze: s.freezeCount, rttMs: round(s.rttMs) });
  rec.warnIf(
    label + ": A/V 차이(비디오−오디오) 평균 |100ms| 이하",
    s.avMean !== null && Math.abs(s.avMean) > 100,
    s.avMean === null ? "측정 불가" : `평균 ${s.avMean.toFixed(1)}ms, 최대|${s.avMaxAbs.toFixed(1)}|ms (${s.avCount}개)`,
  );
  rec.warnIf(
    label + ": 지연(캡처→재생) 평균 500ms 이하",
    s.latMean !== null && s.latMean > 500,
    s.latMean === null ? "측정 불가" : `평균 ${s.latMean.toFixed(1)}ms, 최대 ${s.latMax.toFixed(1)}ms (${s.latCount}개)`,
  );
  rec.addMeasurement({ label, ...Object.fromEntries(Object.entries(s).map(([k, v]) => [k, typeof v === "number" ? round(v, 2) : v])) });
  return s;
}

const round = (v, d = 1) => (v === null || v === undefined ? null : Math.round(v * 10 ** d) / 10 ** d);
