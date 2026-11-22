// 시청 통계: RTCPeerConnection.getStats()에서 보여 줄 값을 뽑는다.
//
// 지연과 A/V 차이는 estimatedPlayoutTimestamp(지금 재생 중인 프레임의 송신 쪽 NTP 시각)로 구한다.
// 서버는 방송자의 캡처 시각을 SR로 그대로 넘기므로(D-036), 방송자와 시청자 PC의 시계가 맞으면
// "지연"은 캡처에서 재생까지 걸린 시간이다. 같은 PC에서는 정확하고, 다른 PC면 방송자·시청자 PC 시계 차이만큼 틀린다.
//
// 손실(packetsLost)은 RTX로 되찾은 패킷도 포함한 원 손실이다(libwebrtc main 기준, WebView2 버전에서 미검증).
// 그래서 손실이 늘어도 재전송(retransmittedPacketsReceived)이 따라 늘면 화면에는 영향이 없을 수 있다.
//
// 원인 가르기(진단용 값): 손실·NACK·재전송은 망, PLI·키프레임은 키프레임 대기,
// 은닉·오디오 손실은 소리 끊김, decodeMsPerFrame·framesDropped는 디코딩 한계를 가리킨다.
//
// 값이 없거나 숫자가 아니면 null이다(0으로 채우지 않는다). 누적 값은 getStats 값 그대로,
// "구간" 값(kbps·jitterMs·decodeMsPerFrame 등)은 직전 측정과의 차로 구한다.

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
  /** 오디오 수신 패킷 누적 수(립싱크 보정이 오디오가 흐르는 구간만 쓰려고 본다) */
  audioPackets: number;
  /** 개발용: 지터 버퍼 목표·최소 지연(동기화가 늘린 값 포함) */
  videoTargetMs: number | null;
  audioTargetMs: number | null;
  /** 오디오 은닉 샘플 누적 수(inbound-rtp `concealedSamples`, 없으면 0). 립싱크 보정이 끊김 사건을 알아채는 데 쓴다 */
  audioConcealedSamples: number;

  // ----- 진단용 비디오 값(inbound-rtp kind=video). 누적이며 없으면 null -----
  videoPacketsLost: number | null;
  videoPacketsReceived: number | null;
  nackCount: number | null;
  pliCount: number | null;
  firCount: number | null;
  /** RTX로 받은 재전송 패킷 누적 수 */
  retransmittedPacketsReceived: number | null;
  keyFramesDecoded: number | null;
  framesReceived: number | null;
  /** 멈춤 누적 시간(ms) */
  totalFreezesDurationMs: number | null;
  pauseCount: number | null;
  totalPausesDurationMs: number | null;
  videoPacketsDiscarded: number | null;
  /** RTP 도착 흔들림(ms). 지터 버퍼 지연(videoJitterMs)과 다르다 */
  videoNetJitterMs: number | null;
  /** 구간 평균 프레임당 디코딩 시간(ms). 구간에 디코딩이 없으면 null */
  decodeMsPerFrame: number | null;
  /** 구간 평균 프레임당 조립 시간(ms, 여러 패킷으로 온 프레임만) */
  assemblyMsPerFrame: number | null;

  // ----- 진단용 오디오 값(inbound-rtp kind=audio) -----
  audioPacketsLost: number | null;
  audioPacketsDiscarded: number | null;
  concealmentEvents: number | null;
  silentConcealedSamples: number | null;
  insertedSamplesForDeceleration: number | null;
  removedSamplesForAcceleration: number | null;
  audioNetJitterMs: number | null;
  totalSamplesReceived: number | null;
  /** 오디오 코덱("opus"·"red" 등). RED를 켰는지 볼 때 쓴다 */
  audioCodec: string | null;
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

/** 직전 측정 이후 구간의 평균 목표 지연(ms). */
function targetMs(cur: Stat | undefined, prev: Stat | undefined): number | null {
  if (!cur) return null;
  const delay = num(cur.jitterBufferTargetDelay);
  const count = num(cur.jitterBufferEmittedCount);
  if (delay === null || count === null) return null;
  const pd = prev ? (num(prev.jitterBufferTargetDelay) ?? 0) : 0;
  const pc = prev ? (num(prev.jitterBufferEmittedCount) ?? 0) : 0;
  const dc = count - pc;
  return dc > 0 ? ((delay - pd) / dc) * 1000 : null;
}

/** 초 단위 값을 ms로. 없으면 null. */
function secToMs(v: unknown): number | null {
  const n = num(v);
  return n === null ? null : n * 1000;
}

/** 직전 측정 이후 구간의 (Δ합계 / Δ개수) × 1000. 개수가 안 늘었거나 직전 값이 없으면 null. */
function perFrameMs(
  cur: Stat | undefined,
  prev: Stat | undefined,
  totalKey: string,
  countKey: string,
): number | null {
  if (!cur || !prev) return null;
  const total = num(cur[totalKey]);
  const count = num(cur[countKey]);
  const pt = num(prev[totalKey]);
  const pc = num(prev[countKey]);
  if (total === null || count === null || pt === null || pc === null) return null;
  const dc = count - pc;
  return dc > 0 ? ((total - pt) / dc) * 1000 : null;
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

  const audioCodecStat = audio?.codecId ? (report.get(String(audio.codecId)) as Stat | undefined) : undefined;
  const audioMime = typeof audioCodecStat?.mimeType === "string" ? audioCodecStat.mimeType : null;

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
    audioPackets: num(audio?.packetsReceived) ?? 0,
    videoTargetMs: targetMs(video, before(video)),
    audioTargetMs: targetMs(audio, before(audio)),
    audioConcealedSamples: num(audio?.concealedSamples) ?? 0,
    videoPacketsLost: num(video?.packetsLost),
    videoPacketsReceived: num(video?.packetsReceived),
    nackCount: num(video?.nackCount),
    pliCount: num(video?.pliCount),
    firCount: num(video?.firCount),
    retransmittedPacketsReceived: num(video?.retransmittedPacketsReceived),
    keyFramesDecoded: num(video?.keyFramesDecoded),
    framesReceived: num(video?.framesReceived),
    totalFreezesDurationMs: secToMs(video?.totalFreezesDuration),
    pauseCount: num(video?.pauseCount),
    totalPausesDurationMs: secToMs(video?.totalPausesDuration),
    videoPacketsDiscarded: num(video?.packetsDiscarded),
    videoNetJitterMs: secToMs(video?.jitter),
    decodeMsPerFrame: perFrameMs(video, before(video), "totalDecodeTime", "framesDecoded"),
    assemblyMsPerFrame: perFrameMs(video, before(video), "totalAssemblyTime", "framesAssembledFromMultiplePackets"),
    audioPacketsLost: num(audio?.packetsLost),
    audioPacketsDiscarded: num(audio?.packetsDiscarded),
    concealmentEvents: num(audio?.concealmentEvents),
    silentConcealedSamples: num(audio?.silentConcealedSamples),
    insertedSamplesForDeceleration: num(audio?.insertedSamplesForDeceleration),
    removedSamplesForAcceleration: num(audio?.removedSamplesForAcceleration),
    audioNetJitterMs: secToMs(audio?.jitter),
    totalSamplesReceived: num(audio?.totalSamplesReceived),
    audioCodec: audioMime ? audioMime.replace(/^audio\//, "") : null,
  };
  return { stats, current };
}
