// 시청 통계: RTCPeerConnection.getStats()에서 보여 줄 값을 뽑는다.
//
// 지연과 A/V 차이는 estimatedPlayoutTimestamp(지금 재생 중인 프레임의 송신 쪽 NTP 시각)로 구한다.
// 서버는 방송자의 캡처 시각을 SR로 그대로 넘기므로(D-036), 방송자와 시청자 PC의 시계가 맞으면
// "지연"은 캡처에서 재생까지 걸린 시간이다. 같은 PC에서는 정확하고, 다른 PC면 시계 차이만큼 틀린다.

export interface WatchStats {
  width: number;
  height: number;
  fps: number | null;
  codec: string | null;
  decoder: string | null;
  videoKbps: number;
  audioKbps: number;
  /** 캡처 → 재생(ms). 송신 SR이 아직 없으면 null */
  latencyMs: number | null;
  /** 비디오 재생 시각 − 오디오 재생 시각(ms). 양수면 비디오가 앞선다 */
  avOffsetMs: number | null;
  videoJitterMs: number | null;
  audioJitterMs: number | null;
  rttMs: number | null;
  packetsLost: number;
  framesDropped: number;
  freezeCount: number;
  framesDecoded: number;
}

/** NTP(1900) → 유닉스(1970) 밀리초 차이 */
const NTP_UNIX_OFFSET_MS = 2208988800000;

type Stat = Record<string, unknown> & { id: string; type: string; timestamp: number };

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

function kbps(cur: Stat | undefined, prev: Stat | undefined): number {
  if (!cur || !prev) return 0;
  const bytes = (num(cur.bytesReceived) ?? 0) - (num(prev.bytesReceived) ?? 0);
  const ms = cur.timestamp - prev.timestamp;
  return ms > 0 ? (bytes * 8) / ms : 0;
}

/** 직전 측정 이후 구간의 평균 지터 버퍼 지연(ms). */
function jitterMs(cur: Stat | undefined, prev: Stat | undefined): number | null {
  if (!cur) return null;
  const delay = num(cur.jitterBufferDelay);
  const count = num(cur.jitterBufferEmittedCount);
  if (delay === null || count === null) return null;
  const pd = prev ? (num(prev.jitterBufferDelay) ?? 0) : 0;
  const pc = prev ? (num(prev.jitterBufferEmittedCount) ?? 0) : 0;
  const dc = count - pc;
  return dc > 0 ? ((delay - pd) / dc) * 1000 : null;
}

function playoutUnixMs(s: Stat | undefined): number | null {
  const ept = num(s?.estimatedPlayoutTimestamp);
  return ept === null || ept <= 0 ? null : ept - NTP_UNIX_OFFSET_MS;
}

export function collectStats(
  report: RTCStatsReport,
  prev: Map<string, RTCStats> | null,
): { stats: WatchStats; current: Map<string, RTCStats> } {
  const current = new Map<string, RTCStats>();
  let video: Stat | undefined;
  let audio: Stat | undefined;
  let pair: Stat | undefined;
  report.forEach((raw) => {
    const s = raw as Stat;
    current.set(s.id, raw);
    if (s.type === "inbound-rtp" && s.kind === "video") video = s;
    if (s.type === "inbound-rtp" && s.kind === "audio") audio = s;
    if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") pair = s;
  });
  const before = (s: Stat | undefined) =>
    s && prev ? (prev.get(s.id) as Stat | undefined) : undefined;

  const codecStat = video?.codecId ? (report.get(String(video.codecId)) as Stat | undefined) : undefined;
  const mime = typeof codecStat?.mimeType === "string" ? codecStat.mimeType : null;
  const decoderName = typeof video?.decoderImplementation === "string" ? video.decoderImplementation : null;
  const hw = video?.powerEfficientDecoder === true ? "HW" : null;

  const videoPlayout = playoutUnixMs(video);
  const audioPlayout = playoutUnixMs(audio);
  const now = video?.timestamp ?? Date.now();
  const rtt = num(pair?.currentRoundTripTime);

  const stats: WatchStats = {
    width: num(video?.frameWidth) ?? 0,
    height: num(video?.frameHeight) ?? 0,
    fps: num(video?.framesPerSecond),
    codec: mime ? mime.replace(/^video\//, "") : null,
    decoder: [decoderName, hw].filter(Boolean).join(", ") || null,
    videoKbps: kbps(video, before(video)),
    audioKbps: kbps(audio, before(audio)),
    latencyMs: videoPlayout !== null ? now - videoPlayout : null,
    avOffsetMs: videoPlayout !== null && audioPlayout !== null ? videoPlayout - audioPlayout : null,
    videoJitterMs: jitterMs(video, before(video)),
    audioJitterMs: jitterMs(audio, before(audio)),
    rttMs: rtt !== null ? rtt * 1000 : null,
    packetsLost: (num(video?.packetsLost) ?? 0) + (num(audio?.packetsLost) ?? 0),
    framesDropped: num(video?.framesDropped) ?? 0,
    freezeCount: num(video?.freezeCount) ?? 0,
    framesDecoded: num(video?.framesDecoded) ?? 0,
  };
  return { stats, current };
}
