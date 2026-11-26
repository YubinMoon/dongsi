// 배포 빌드의 시청 로그 로직. DOM·시계 없이 통계를 받아 앱 로그에 남길 줄을 돌려준다(시각은 인자).
//
// - 사건 줄(`watch-event `): 멈춤·키프레임 요청·소리 끊김·버퍼 변경·창 숨김을 일이 있을 때 한 줄씩.
//   한 요약 구간에 `maxEventsPerWindow`개까지만 내고, 넘친 수는 그 구간의 요약 줄에 `suppressed=N`으로 적는다.
// - 요약 줄(`watch-sum `): 첫 통계 뒤 `summaryMs`마다 한 줄. 구간 증가량·평균과 지금 값.
//
// 손실(packetsLost)은 RTX로 되찾은 패킷도 포함한 원 손실이다(libwebrtc main 기준, WebView2 버전에서 미검증).
// 캡처→재생 지연은 방송자·시청자 PC 시계 차이가 섞여 있어 키 이름을 `latency_clock`으로 둔다.
// 오디오 RED(D-087)면 `a_discard`·audio-gap의 `discarded`는 RED 중복 블록이 섞인 값이고(초당 약 195개), 소리 손실(lost의 뒤쪽)은 RED가 복구해도 늘어난다.
// 복구됐는지는 `conceal_ms`로 본다. 카운터가 줄어들면(새 연결 등) 그 구간의 증가량은 0으로 보고 기준만 새로 잡는다. 값이 없으면 `-`.
// 줄 하나는 600자 안쪽이다.

import type { WatchStats } from "./watchStats";

export interface WatchLogInput {
  /** 통계를 읽은 시각(ms) */
  now: number;
  stats: WatchStats;
  /** 시청 버퍼 B(ms). 모르면 null */
  bufferMs: number | null;
  /** 창이 숨겨져 있는가 */
  hidden: boolean;
  /** 이번 통계에서 앱이 키프레임을 요청했는가 */
  keyframeRequested: boolean;
  /** 오디오 RED를 협상했는가(D-087). `red=0|1`로 남는다 */
  audioRed: boolean;
}

export interface WatchLoggerOptions {
  /** 요약 줄 간격(ms). 기본 10000 */
  summaryMs?: number;
  /** 한 요약 구간에 낼 사건 줄의 상한. 기본 10 */
  maxEventsPerWindow?: number;
}

/** 오디오 은닉이 이 길이(48kHz 2400샘플) 이상이면 audio-gap 사건 */
export const AUDIO_GAP_MS = 50;
const SAMPLES_PER_MS = 48;
/** Opus 패킷 하나의 길이(ms). 손실 패킷 수 → 손실로 설명되는 소리 길이 */
const AUDIO_PACKET_MS = 10;
const MAX_LINE = 600;

/** 누적 값이 늘어난 양. 어느 쪽이든 없으면 null, 줄어들었으면(새 연결 등) 0. */
function delta(cur: number | null, prev: number | null): number | null {
  if (cur === null || prev === null) return null;
  return cur >= prev ? cur - prev : 0;
}

function num(n: number | null | undefined, digits = 0): string {
  return n === null || n === undefined || !Number.isFinite(n) ? "-" : n.toFixed(digits);
}

/** 공백·따옴표·= 가 든 문자열은 따옴표로 감싼다. 없으면 "-". */
function str(s: string | null | undefined): string {
  if (s === null || s === undefined || s === "") return "-";
  return /[\s"=]/.test(s) ? JSON.stringify(s) : s;
}

function clip(line: string): string {
  return line.length > MAX_LINE ? line.slice(0, MAX_LINE) : line;
}

/** 구간 증가량 합계. 한 번도 값이 없었으면 null(로그에는 `-`). */
class Sum {
  private total = 0;
  private seen = false;
  add(v: number | null): void {
    if (v === null) return;
    this.total += v;
    this.seen = true;
  }
  get value(): number | null {
    return this.seen ? this.total : null;
  }
}

class Avg {
  private total = 0;
  private count = 0;
  add(v: number | null): void {
    if (v === null || !Number.isFinite(v)) return;
    this.total += v;
    this.count += 1;
  }
  get value(): number | null {
    return this.count > 0 ? this.total / this.count : null;
  }
}

class Window {
  start: number;
  samples = 0;
  events = 0;
  suppressed = 0;
  lostV = new Sum();
  lostA = new Sum();
  nack = new Sum();
  rtx = new Sum();
  pli = new Sum();
  key = new Sum();
  freezes = new Sum();
  freezeMs = new Sum();
  dropped = new Sum();
  concealMs = new Sum();
  aDiscard = new Sum();
  fps = new Avg();
  vKbps = new Avg();
  aKbps = new Avg();
  constructor(start: number) {
    this.start = start;
  }
}

export class WatchLogger {
  private readonly summaryMs: number;
  private readonly maxEvents: number;
  private prev: WatchStats | null = null;
  private win: Window | null = null;
  private lastBuffer: number | null | undefined;
  private lastHidden: boolean | undefined;

  constructor(opts: WatchLoggerOptions = {}) {
    this.summaryMs = opts.summaryMs ?? 10_000;
    this.maxEvents = opts.maxEventsPerWindow ?? 10;
  }

  /** 새 시청(재접속 포함)을 시작할 때 모든 기준을 지운다. */
  reset(): void {
    this.prev = null;
    this.win = null;
    this.lastBuffer = undefined;
    this.lastHidden = undefined;
  }

  /** 이번 통계에서 남길 줄들. 없으면 []. */
  update(input: WatchLogInput): string[] {
    const { now, stats: s, bufferMs, hidden, keyframeRequested, audioRed } = input;
    const prev = this.prev;
    const win = (this.win ??= new Window(now));
    const lines: string[] = [];
    const event = (line: string): void => {
      if (win.events >= this.maxEvents) {
        win.suppressed += 1;
        return;
      }
      win.events += 1;
      lines.push(clip(`watch-event ${line}`));
    };

    win.samples += 1;
    const d = {
      lostV: delta(s.videoPacketsLost, prev?.videoPacketsLost ?? null),
      lostA: delta(s.audioPacketsLost, prev?.audioPacketsLost ?? null),
      nack: delta(s.nackCount, prev?.nackCount ?? null),
      rtx: delta(s.retransmittedPacketsReceived, prev?.retransmittedPacketsReceived ?? null),
      pli: delta(s.pliCount, prev?.pliCount ?? null),
      key: delta(s.keyFramesDecoded, prev?.keyFramesDecoded ?? null),
      freezes: prev ? delta(s.freezeCount, prev.freezeCount) : null,
      freezeMs: delta(s.totalFreezesDurationMs, prev?.totalFreezesDurationMs ?? null),
      dropped: prev ? delta(s.framesDropped, prev.framesDropped) : null,
      concealed: prev ? delta(s.audioConcealedSamples, prev.audioConcealedSamples) : null,
      aDiscard: delta(s.audioPacketsDiscarded, prev?.audioPacketsDiscarded ?? null),
    };
    const concealMs = d.concealed === null ? null : d.concealed / SAMPLES_PER_MS;

    win.lostV.add(d.lostV);
    win.lostA.add(d.lostA);
    win.nack.add(d.nack);
    win.rtx.add(d.rtx);
    win.pli.add(d.pli);
    win.key.add(d.key);
    win.freezes.add(d.freezes);
    win.freezeMs.add(d.freezeMs);
    win.dropped.add(d.dropped);
    win.concealMs.add(concealMs);
    win.aDiscard.add(d.aDiscard);
    win.fps.add(s.fps);
    if (prev) {
      // 첫 통계의 kbps는 직전 측정이 없어 0이다. 평균에서 뺀다.
      win.vKbps.add(s.videoKbps);
      win.aKbps.add(s.audioKbps);
    }

    // ----- 사건 -----
    if (d.freezes !== null && d.freezes > 0) {
      event(
        `freeze n=${d.freezes} dur_ms=${num(d.freezeMs)} lost_v=${num(d.lostV)} lost_a=${num(d.lostA)}` +
          ` nack=${num(d.nack)} rtx=${num(d.rtx)} pli=${num(d.pli)} fps=${num(s.fps)}`,
      );
    }
    if (keyframeRequested) {
      event(`keyframe-request frames_decoded=${s.framesDecoded} kbps=${num(s.videoKbps)} pli=${num(s.pliCount)}`);
    }
    if (concealMs !== null && concealMs >= AUDIO_GAP_MS) {
      const lostMs = d.lostA === null ? null : d.lostA * AUDIO_PACKET_MS;
      event(
        `audio-gap concealed_ms=${num(concealMs)} lost_ms=${num(lostMs)} discarded=${num(d.aDiscard)} red=${audioRed ? 1 : 0}`,
      );
    }
    const buf = bufferMs === null ? null : Math.round(bufferMs);
    if (this.lastBuffer !== undefined && buf !== this.lastBuffer) {
      event(`buffer from=${num(this.lastBuffer)} to=${num(buf)}`);
    }
    this.lastBuffer = buf;
    if (this.lastHidden === undefined ? hidden : hidden !== this.lastHidden) {
      event(`hidden hidden=${hidden}`);
    }
    this.lastHidden = hidden;

    // ----- 요약 -----
    if (now - win.start >= this.summaryMs) {
      lines.push(this.summary(win, now, input));
      this.win = new Window(now);
    }
    this.prev = s;
    return lines;
  }

  private summary(w: Window, now: number, input: WatchLogInput): string {
    const s = input.stats;
    return clip(
      `watch-sum win_s=${((now - w.start) / 1000).toFixed(1)}` +
        ` lost=${num(w.lostV.value)}/${num(w.lostA.value)} nack=${num(w.nack.value)} rtx=${num(w.rtx.value)}` +
        ` pli=${num(w.pli.value)} key=${num(w.key.value)} freeze=${num(w.freezes.value)}/${num(w.freezeMs.value)}ms` +
        ` dropped=${num(w.dropped.value)} conceal_ms=${num(w.concealMs.value)} a_discard=${num(w.aDiscard.value)}` +
        ` suppressed=${w.suppressed}` +
        ` fps=${num(w.fps.value, 1)} kbps=${num(w.vKbps.value)}+${num(w.aKbps.value)}` +
        ` res=${s.width}x${s.height} codec=${str(s.codec)} decoder=${str(s.decoder)} dec_ms=${num(s.decodeMsPerFrame, 1)}` +
        ` jb=${num(s.videoJitterMs)}/${num(s.audioJitterMs)} latency_clock=${num(s.latencyMs)} rtt=${num(s.rttMs)}` +
        ` buf=${num(input.bufferMs)} hidden=${input.hidden} red=${input.audioRed ? 1 : 0}`,
    );
  }
}
