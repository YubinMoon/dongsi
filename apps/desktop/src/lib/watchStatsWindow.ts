// 시청 통계의 최근 구간(10초) 증분·시계열(60샘플) 추이·상태 판정·AI 진단 복사 포맷.
// DOM·시계 없이 순수 로직으로 동작한다(시각은 호출하는 쪽의 단조 시계 ms).

import { PLAYOUT_BUFFER_DEFAULTS } from "./playoutBuffer";
import { AUDIO_GAP_MS } from "./watchLog";
import { viewerLatencyMs } from "./viewerLatency";
import type { WatchStats } from "./watchStats";

/** 최근 구간 집계의 창 길이(ms) */
export const RECENT_WINDOW_MS = 10_000;
/** 스파크라인·추이 요약에 보관할 최대 샘플 수 */
export const HISTORY_LIMIT = 60;
const SAMPLES_PER_MS = 48;
const BUFFER_BASE_MS = PLAYOUT_BUFFER_DEFAULTS.baseMs;

export type HealthTone = "ok" | "warn" | "err";

export interface WatchHealth {
  tone: HealthTone;
  /** 패널 헤더 배지에 표시할 짧은 상태 요약 */
  label: string;
  /** AI 복사·진단용 활성 조건 목록(영문 식별자 + 핵심 수치) */
  reasons: string[];
}

export interface RecentStats {
  /** 집계 창이 덮는 실제 시간(초). 샘플이 1개면 0 */
  spanSec: number;
  videoPacketsLost: number | null;
  videoPacketsReceived: number | null;
  /** 최근 구간 영상 손실률(%). 수신+손실 합이 0이면 null */
  videoLossRatePct: number | null;
  /** RTX 재전송으로 설명되지 않은 영상 손실 추정치(최소 0) */
  videoUnrecoveredLost: number | null;
  nackCount: number | null;
  retransmittedPackets: number | null;
  pliCount: number | null;
  freezes: number;
  freezeMs: number;
  framesDropped: number;
  keyframeRequests: number;
  audioPacketsLost: number | null;
  /** 최근 구간 오디오 은닉 길이(ms, 48kHz 환산) */
  audioConcealMs: number;
  audioConcealEvents: number | null;
}

export interface StatsHistory {
  videoMbps: (number | null)[];
  rttMs: (number | null)[];
  fps: (number | null)[];
  videoJitterMs: (number | null)[];
}

export interface SeriesSummary {
  min: number;
  avg: number;
  max: number;
}

export interface WatchStatsSnapshot {
  recent: RecentStats;
  history: StatsHistory;
  health: WatchHealth;
}

export interface TrackerInput {
  now: number;
  stats: WatchStats;
  bufferMs: number;
  /** 방송자가 송출 중인 목표 fps(알면 전달). 디코딩 예산 판정에 쓴다 */
  targetFps?: number | null;
  audioRed?: boolean;
  keyframeRequested?: boolean;
}

interface StepEntry {
  now: number;
  lostV: number | null;
  recvV: number | null;
  nack: number | null;
  rtx: number | null;
  pli: number | null;
  freezes: number;
  freezeMs: number;
  dropped: number;
  keyframeReq: number;
  lostA: number | null;
  concealMs: number;
  concealEvents: number | null;
}

/** 누적 카운터 증가량. 어느 쪽이든 없으면 null, 카운터가 줄었으면(재연결·디코더 재생성) 0. */
function delta(cur: number | null, prev: number | null): number | null {
  if (cur === null || prev === null) return null;
  return cur >= prev ? cur - prev : 0;
}

function sumNullable(entries: StepEntry[], pick: (e: StepEntry) => number | null): number | null {
  let total = 0;
  let seen = false;
  for (const e of entries) {
    const v = pick(e);
    if (v === null) continue;
    total += v;
    seen = true;
  }
  return seen ? total : null;
}

function sumNum(entries: StepEntry[], pick: (e: StepEntry) => number): number {
  let total = 0;
  for (const e of entries) total += pick(e);
  return total;
}

/** 숫자 배열(null 제외)의 min/avg/max. 유효한 값이 없으면 null. */
export function summarizeSeries(values: readonly (number | null)[]): SeriesSummary | null {
  let min = Infinity;
  let max = -Infinity;
  let sum = 0;
  let count = 0;
  for (const v of values) {
    if (v === null || !Number.isFinite(v)) continue;
    if (v < min) min = v;
    if (v > max) max = v;
    sum += v;
    count += 1;
  }
  return count > 0 ? { min, avg: sum / count, max } : null;
}

/** 현재 통계 + 최근 10초 구간 지표로 상태 톤과 원인 요약을 판정한다. */
export function evaluateHealth(input: {
  stats: WatchStats;
  recent: RecentStats;
  bufferMs: number;
  targetFps?: number | null;
}): WatchHealth {
  const { stats: s, recent: r, bufferMs } = input;
  const reasons: string[] = [];
  let errLabel: string | null = null;
  let warnLabel: string | null = null;

  // ----- 문제(err): 체감 끊김 또는 미복구 손실 -----
  if (r.freezes > 0 || r.keyframeRequests > 0) {
    errLabel ??= "화면 멈춤";
    reasons.push(
      `video_freeze(count=${r.freezes},dur_ms=${Math.round(r.freezeMs)},kf_req=${r.keyframeRequests})`,
    );
  }
  if (r.audioConcealMs >= AUDIO_GAP_MS) {
    errLabel ??= "소리 끊김";
    reasons.push(`audio_gap(conceal_ms=${Math.round(r.audioConcealMs)})`);
  }
  const unrecovered = r.videoUnrecoveredLost ?? r.videoPacketsLost ?? 0;
  if (r.videoLossRatePct !== null && r.videoLossRatePct >= 2 && unrecovered > 0) {
    errLabel ??= "패킷 손실";
    reasons.push(`video_loss(${r.videoLossRatePct.toFixed(1)}%,unrecovered_est=${unrecovered})`);
  }

  // ----- 주의(warn): 자동 완충·복구 중·디코딩 부담·동기 이상 -----
  if (bufferMs > BUFFER_BASE_MS) {
    warnLabel ??= "버퍼 확장 중";
    reasons.push(`buffer_expanded(${Math.round(bufferMs)}ms>${BUFFER_BASE_MS}ms)`);
  }
  const fpsBudgetBase = input.targetFps && input.targetFps > 0 ? input.targetFps : s.fps;
  if (s.decodeMsPerFrame !== null && fpsBudgetBase !== null && fpsBudgetBase > 0) {
    const budgetMs = 1000 / fpsBudgetBase;
    if (s.decodeMsPerFrame > budgetMs * 0.75) {
      warnLabel ??= "디코딩 부담";
      reasons.push(`decode_load(${s.decodeMsPerFrame.toFixed(1)}ms/${budgetMs.toFixed(1)}ms)`);
    }
  }
  if (
    ((r.videoPacketsLost ?? 0) > 0 && (r.retransmittedPackets ?? 0) > 0) ||
    (r.videoLossRatePct !== null && r.videoLossRatePct >= 0.5)
  ) {
    warnLabel ??= "패킷 복구 중";
    reasons.push(`packet_recovery(lost=${r.videoPacketsLost ?? 0},rtx=${r.retransmittedPackets ?? 0})`);
  }
  if (s.avOffsetMs !== null && Math.abs(s.avOffsetMs) >= 60) {
    warnLabel ??= "A/V 어긋남";
    reasons.push(`av_offset(${Math.round(s.avOffsetMs)}ms)`);
  }

  if (errLabel) return { tone: "err", label: errLabel, reasons };
  if (warnLabel) return { tone: "warn", label: warnLabel, reasons };
  return { tone: "ok", label: "정상", reasons: [] };
}

export class WatchStatsTracker {
  private prev: WatchStats | null = null;
  private steps: StepEntry[] = [];
  private firstInWindowAt: number | null = null;
  private videoMbps: (number | null)[] = [];
  private rttMs: (number | null)[] = [];
  private fps: (number | null)[] = [];
  private videoJitterMs: (number | null)[] = [];

  reset(): void {
    this.prev = null;
    this.steps = [];
    this.firstInWindowAt = null;
    this.videoMbps = [];
    this.rttMs = [];
    this.fps = [];
    this.videoJitterMs = [];
  }

  push(input: TrackerInput): WatchStatsSnapshot {
    const { now, stats: s, bufferMs, targetFps } = input;
    const prev = this.prev;

    if (prev) {
      const concealSamples = delta(s.audioConcealedSamples, prev.audioConcealedSamples) ?? 0;
      this.steps.push({
        now,
        lostV: delta(s.videoPacketsLost, prev.videoPacketsLost),
        recvV: delta(s.videoPacketsReceived, prev.videoPacketsReceived),
        nack: delta(s.nackCount, prev.nackCount),
        rtx: delta(s.retransmittedPacketsReceived, prev.retransmittedPacketsReceived),
        pli: delta(s.pliCount, prev.pliCount),
        freezes: delta(s.freezeCount, prev.freezeCount) ?? 0,
        freezeMs: delta(s.totalFreezesDurationMs, prev.totalFreezesDurationMs) ?? 0,
        dropped: delta(s.framesDropped, prev.framesDropped) ?? 0,
        keyframeReq: input.keyframeRequested ? 1 : 0,
        lostA: delta(s.audioPacketsLost, prev.audioPacketsLost),
        concealMs: concealSamples / SAMPLES_PER_MS,
        concealEvents: delta(s.concealmentEvents, prev.concealmentEvents),
      });
    } else {
      this.firstInWindowAt = now;
    }
    this.prev = s;

    const cutoff = now - RECENT_WINDOW_MS;
    while (this.steps.length > 0 && this.steps[0].now <= cutoff) {
      this.firstInWindowAt = this.steps[0].now;
      this.steps.shift();
    }

    const pushLimited = (arr: (number | null)[], v: number | null) => {
      arr.push(v);
      if (arr.length > HISTORY_LIMIT) arr.shift();
    };
    pushLimited(this.videoMbps, s.videoKbps === null ? null : s.videoKbps / 1000);
    pushLimited(this.rttMs, s.rttMs);
    pushLimited(this.fps, s.fps);
    pushLimited(this.videoJitterMs, s.videoJitterMs);

    const lostV = sumNullable(this.steps, (e) => e.lostV);
    const recvV = sumNullable(this.steps, (e) => e.recvV);
    const rtx = sumNullable(this.steps, (e) => e.rtx);
    const totalV = (lostV ?? 0) + (recvV ?? 0);
    const videoLossRatePct =
      lostV !== null && recvV !== null && totalV > 0 ? (lostV / totalV) * 100 : null;
    const videoUnrecoveredLost =
      lostV !== null && rtx !== null ? Math.max(0, lostV - rtx) : null;

    const spanStart = this.firstInWindowAt ?? now;
    const spanSec = Math.max(0, Math.min(RECENT_WINDOW_MS, now - spanStart) / 1000);

    const recent: RecentStats = {
      spanSec,
      videoPacketsLost: lostV,
      videoPacketsReceived: recvV,
      videoLossRatePct,
      videoUnrecoveredLost,
      nackCount: sumNullable(this.steps, (e) => e.nack),
      retransmittedPackets: rtx,
      pliCount: sumNullable(this.steps, (e) => e.pli),
      freezes: sumNum(this.steps, (e) => e.freezes),
      freezeMs: sumNum(this.steps, (e) => e.freezeMs),
      framesDropped: sumNum(this.steps, (e) => e.dropped),
      keyframeRequests: sumNum(this.steps, (e) => e.keyframeReq),
      audioPacketsLost: sumNullable(this.steps, (e) => e.lostA),
      audioConcealMs: sumNum(this.steps, (e) => e.concealMs),
      audioConcealEvents: sumNullable(this.steps, (e) => e.concealEvents),
    };

    const history: StatsHistory = {
      videoMbps: [...this.videoMbps],
      rttMs: [...this.rttMs],
      fps: [...this.fps],
      videoJitterMs: [...this.videoJitterMs],
    };

    const health = evaluateHealth({ stats: s, recent, bufferMs, targetFps });
    return { recent, history, health };
  }
}

// ---------- AI 진단용 복사 포맷 ----------

export interface AiCopyInput {
  stats: WatchStats;
  snapshot: WatchStatsSnapshot;
  bufferMs: number;
  audioRed: boolean;
  streamIds: string[];
  broadcasterName?: string | null;
  serverName?: string | null;
  targetWidth?: number | null;
  targetHeight?: number | null;
  targetFps?: number | null;
  /** ISO 시각 문자열(테스트 결정론을 위해 주입 가능) */
  capturedAt?: string;
}

function n(v: number | null | undefined, digits = 0): string {
  return v === null || v === undefined || !Number.isFinite(v) ? "null" : v.toFixed(digits);
}

function signed(v: number | null | undefined, digits = 0): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "null";
  const s = v.toFixed(digits);
  return v > 0 ? `+${s}` : s;
}

function q(s: string | null | undefined): string {
  return s ? JSON.stringify(s) : "null";
}

function fmtSummary(label: string, s: SeriesSummary | null, digits: number): string {
  if (!s) return `${label}: null`;
  return `${label}: min=${s.min.toFixed(digits)} avg=${s.avg.toFixed(digits)} max=${s.max.toFixed(digits)}`;
}

/**
 * AI에게 그대로 붙여 넣어 스트리밍 문제(망 손실·RTX·RED·버퍼·디코딩·A/V)를
 * 즉시 진단받을 수 있는 구조화 텍스트를 만든다.
 */
export function formatStatsForAi(input: AiCopyInput): string {
  const { stats: s, snapshot, bufferMs, audioRed } = input;
  const { recent: r, history: h, health } = snapshot;
  const target =
    input.targetWidth && input.targetHeight && input.targetFps
      ? `${input.targetWidth}x${input.targetHeight}@${input.targetFps}fps`
      : "unknown";
  const capturedAt = input.capturedAt ?? new Date().toISOString();

  const lines = [
    "[dongsi watch diagnostics]",
    `captured_at: ${capturedAt}`,
    `stream: broadcaster=${q(input.broadcasterName)} server=${q(input.serverName)} target=${target} audio_red=${audioRed} route=${q(s.connectionRoute)}`,
    `health: status=${health.tone} label=${q(health.label)} reasons=[${health.reasons.join(", ")}]`,
    "",
    "[current]",
    `video: res=${s.width}x${s.height} fps=${n(s.fps)} codec=${s.codec ?? "null"} decoder=${q(s.decoder)} bitrate_kbps=${n(s.videoKbps)} decode_ms=${n(s.decodeMsPerFrame, 1)} assembly_ms=${n(s.assemblyMsPerFrame, 1)}`,
    `audio: codec=${s.audioCodec ?? "null"} bitrate_kbps=${n(s.audioKbps)} red=${audioRed}`,
    `timing: viewer_latency_ms=${n(viewerLatencyMs(s))} av_offset_ms=${signed(s.avOffsetMs)} rtt_ms=${n(s.rttMs, 1)} video_jb_ms=${n(s.videoJitterMs)} audio_jb_ms=${n(s.audioJitterMs)} playout_buffer_ms=${Math.round(bufferMs)} (base=${BUFFER_BASE_MS}) video_net_jitter_ms=${n(s.videoNetJitterMs, 1)} audio_net_jitter_ms=${n(s.audioNetJitterMs, 1)}`,
    "",
    `[recent_10s] (span=${r.spanSec.toFixed(1)}s)`,
    `video: loss_pct=${r.videoLossRatePct === null ? "null" : `${r.videoLossRatePct.toFixed(2)}%`} lost=${n(r.videoPacketsLost)} recv=${n(r.videoPacketsReceived)} unrecovered_est=${n(r.videoUnrecoveredLost)} nack=${n(r.nackCount)} rtx=${n(r.retransmittedPackets)} pli=${n(r.pliCount)} dropped=${r.framesDropped} freezes=${r.freezes} freeze_ms=${Math.round(r.freezeMs)} keyframe_req=${r.keyframeRequests}`,
    `audio: lost=${n(r.audioPacketsLost)} conceal_ms=${Math.round(r.audioConcealMs)} conceal_events=${n(r.audioConcealEvents)}`,
    "",
    `[trend_60s] (samples=${h.videoMbps.length})`,
    fmtSummary("video_mbps", summarizeSeries(h.videoMbps), 2),
    fmtSummary("fps", summarizeSeries(h.fps), 1),
    fmtSummary("rtt_ms", summarizeSeries(h.rttMs), 1),
    fmtSummary("video_jb_ms", summarizeSeries(h.videoJitterMs), 0),
    "",
    "[cumulative]",
    `video: frames_recv=${n(s.framesReceived)} frames_decoded=${s.framesDecoded} dropped=${s.framesDropped} freezes=${s.freezeCount} freeze_ms=${n(s.totalFreezesDurationMs)} pauses=${n(s.pauseCount)} pause_ms=${n(s.totalPausesDurationMs)} keyframes=${n(s.keyFramesDecoded)} pkts_recv=${n(s.videoPacketsReceived)} pkts_lost=${n(s.videoPacketsLost)} pkts_discarded=${n(s.videoPacketsDiscarded)} nack=${n(s.nackCount)} rtx=${n(s.retransmittedPacketsReceived)} pli=${n(s.pliCount)} fir=${n(s.firCount)}`,
    `audio: pkts_recv=${s.audioPackets} pkts_lost=${n(s.audioPacketsLost)} pkts_discarded=${n(s.audioPacketsDiscarded)} conceal_events=${n(s.concealmentEvents)} conceal_samples=${s.audioConcealedSamples} silent_conceal_samples=${n(s.silentConcealedSamples)} decel_samples=${n(s.insertedSamplesForDeceleration)} accel_samples=${n(s.removedSamplesForAcceleration)} total_samples=${n(s.totalSamplesReceived)}`,
    `streams: ${input.streamIds.length > 0 ? input.streamIds.join(" ") : "none"}`,
  ];
  return lines.join("\n");
}
