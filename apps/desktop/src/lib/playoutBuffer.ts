// 시청 버퍼(D-072, 기본값 D-080): 비디오·오디오 두 수신기에 같이 거는 최소 재생 지연(`jitterBufferTarget`) B를 정한다.
//
// 기본 0ms에서 시작한다(D-080, AWS IVS Real-Time의 기본 지터 버퍼 0과 같음: 바닥 없이 브라우저 지터 버퍼(NetEq)의
// 자체 목표만 쓴다. 예전 기본 100ms는 오디오 지연의 가장 큰 몫이었다). 재생이 끊기면(패킷이 버퍼보다 늦게 와서 화면이 멈추거나 소리가 비면) 늦은 만큼 B를 올리고
// (한 번에 최대 100ms, 전체 최대 300ms), 끊김 없이 조용한 시간이 이어지면 15ms(60fps에서 약 1프레임)씩 기본값까지 내린다.
// 내린 뒤 곧 다시 끊기면(되돌이) 다음에 내리기까지 기다리는 시간을 두 배로 늘린다(최대 5분). 오래 조용하면 다시 짧게.
//
// 두 수신기에 같은 B를 거는 까닭: Wi-Fi 흔들림은 두 트랙에 똑같이 오고, 한쪽만 올리면 립싱크 보정이 다른 쪽을 따라
// 올리는 몇 초 동안 A/V가 어긋난다. 립싱크 보정(lipsync.ts)은 B 위에 얹는 차이를 낸다(+page.svelte가 더해서 건다).
//
// 끊김 신호(호출하는 쪽이 재서 `stall`로 넣는다):
// - 비디오: `requestVideoFrameCallback`의 연속한 두 프레임에서 "표시 간격 − 송신 간격(RTP 시각)"(`videoStallMs`).
//   제때 받았는데 늦게 나온 것(디코딩·그리기 쪽 멈춤, 받은 간격 − 송신 간격이 작음)은 버퍼로 막을 수 없어 세지 않는다.
//   화면이 멈춰 있으면 방송자가 새 프레임을 보내지 않으므로(crates/media video.rs) 표시 간격만으로는 끊김을 알 수 없다.
//   브라우저의 `freezeCount`도 같은 까닭으로 정지 화면을 멈춤으로 셀 수 있어(추정) 쓰지 않는다.
// - 오디오: 통계 구간의 은닉 샘플 증가(`audioLateMs`). 방송자는 소리가 없어도 무음 패킷을 계속 보내므로(crates/media audio)
//   은닉은 곧 패킷이 늦었거나 빠졌다는 뜻이다.
//
// 순수 로직이다(시계·DOM 없음). 시각은 호출하는 쪽의 단조 시계(ms, performance.now)다.

export interface PlayoutBufferOptions {
  /** 기본값이자 최솟값(ms) */
  baseMs: number;
  /** 최댓값(ms). 목표 지연 500ms 안에 들게 둔다 */
  maxMs: number;
  /** 이보다 덜 늦은 것은 끊김으로 보지 않는다(ms). 화면 표시는 주사율 단위(60Hz면 16.7ms)로 흔들린다 */
  minStallMs: number;
  /** 이보다 더 늦은 것은 흔들림이 아니라 멈춤·중단으로 보고 무시한다(ms). 버퍼로 막을 수 있는 크기가 아니다 */
  maxStallMs: number;
  /** 한 번에 올리는 최대량(ms) */
  maxRaiseStepMs: number;
  /** 올린 뒤 이 시간 안의 끊김은 같은 흔들림으로 보고 하나로 묶는다(ms). 버퍼를 올리면서 생긴 멈춤도 여기에 묶인다 */
  mergeMs: number;
  /** 처음 이 시간 안의 끊김은 보지 않는다(ms). 시청 시작 직후의 은닉·멈춤은 정상이다 */
  startupMs: number;
  /** 이만큼 끊김 없이 조용하면 한 단계 내린다(ms). 되돌이가 있으면 두 배씩 늘어난다 */
  quietMs: number;
  /** 늘어난 기다림의 최댓값(ms) */
  maxQuietMs: number;
  /** 한 번에 내리는 양(ms) */
  lowerStepMs: number;
  /** 내린 뒤 이 시간 안에 끊기면 되돌이로 보고 기다림을 두 배로(ms) */
  relapseMs: number;
  /** 이만큼 끊김이 없으면 늘어난 기다림을 처음 값으로 돌린다(ms) */
  calmResetMs: number;
}

export const PLAYOUT_BUFFER_DEFAULTS: PlayoutBufferOptions = {
  baseMs: 0,
  maxMs: 300,
  minStallMs: 50,
  maxStallMs: 2000,
  maxRaiseStepMs: 100,
  mergeMs: 2000,
  startupMs: 2000,
  quietMs: 20_000,
  maxQuietMs: 300_000,
  lowerStepMs: 15,
  relapseMs: 60_000,
  calmResetMs: 600_000,
};

/** 끊김을 알아챈 트랙 */
export type StallKind = "video" | "audio";

/** 개발용 기록 한 줄 */
export interface PlayoutBufferEvent {
  t: number;
  /** stall: 끊김(올렸든 아니든), lower: 내림 */
  type: "stall" | "lower";
  kind: StallKind | null;
  lateMs: number | null;
  /** 이 일 뒤의 B(ms) */
  ms: number;
  /** 이 일 뒤의 기다림(ms) */
  quietMs: number;
}

export class PlayoutBuffer {
  private readonly opt: PlayoutBufferOptions;
  private value: number;
  private quiet: number;
  private firstAt: number | null = null;
  private ignoreUntil = -Infinity;
  private lastStallAt: number | null = null;
  private lastChangeAt: number | null = null;
  /** 마지막으로 내린 시각. 되돌이를 한 번 세면 지운다 */
  private lastLowerAt: number | null = null;
  /** 지금 묶음(`mergeMs`)을 시작한 시각과 그때의 B */
  private raiseAt: number | null = null;
  private raiseFrom = 0;
  private log: PlayoutBufferEvent[] = [];

  constructor(options: Partial<PlayoutBufferOptions> = {}) {
    this.opt = { ...PLAYOUT_BUFFER_DEFAULTS, ...options };
    this.value = this.opt.baseMs;
    this.quiet = this.opt.quietMs;
  }

  /** 지금 B(ms) */
  get ms(): number {
    return this.value;
  }

  /** 지금 내리기까지 기다리는 시간(ms) */
  get quietMs(): number {
    return this.quiet;
  }

  /** 개발용: 최근 기록(최대 100개) */
  get events(): readonly PlayoutBufferEvent[] {
    return this.log;
  }

  /** 새 시청처럼 처음부터 */
  reset(): void {
    this.value = this.opt.baseMs;
    this.quiet = this.opt.quietMs;
    this.firstAt = null;
    this.ignoreUntil = -Infinity;
    this.lastStallAt = null;
    this.lastChangeAt = null;
    this.lastLowerAt = null;
    this.raiseAt = null;
    this.raiseFrom = 0;
    this.log = [];
  }

  /**
   * `now`부터 `ms` 동안 끊김을 보지 않는다. 립싱크 보정이 바닥을 올렸을 때(그만큼 화면이 멈추거나 소리가 늘어난다),
   * 코덱·해상도가 바뀌었을 때, 창이 숨겨졌다 돌아왔을 때 부른다.
   */
  hold(now: number, ms: number): void {
    this.ignoreUntil = Math.max(this.ignoreUntil, now + ms);
  }

  /**
   * 끊김 하나를 넣는다. `lateMs`는 늦은 정도(ms). B를 올렸으면 새 B를, 아니면 null을 돌려준다.
   * 무시하는 경우: 시작 직후, `hold` 동안, `minStallMs` 미만, `maxStallMs` 초과, 숫자가 아님.
   */
  stall(now: number, lateMs: number, kind: StallKind): number | null {
    const o = this.opt;
    this.firstAt ??= now;
    if (!Number.isFinite(lateMs) || lateMs < o.minStallMs || lateMs > o.maxStallMs) return null;
    if (now - this.firstAt < o.startupMs || now < this.ignoreUntil) return null;

    // 되돌이: 내린 뒤 곧 다시 끊겼다. 내림 한 번에 한 번만 센다.
    if (this.lastLowerAt !== null && now - this.lastLowerAt < o.relapseMs) {
      this.quiet = Math.min(this.quiet * 2, o.maxQuietMs);
    }
    this.lastLowerAt = null;
    this.lastStallAt = now;

    // 묶음: 올린 지 얼마 안 됐으면 그 묶음을 시작할 때의 B에서 다시 잰다(같은 흔들림으로 두 번 올리지 않는다).
    if (this.raiseAt === null || now - this.raiseAt >= o.mergeMs) {
      this.raiseAt = now;
      this.raiseFrom = this.value;
    }
    const target = Math.min(o.maxMs, Math.round(this.raiseFrom + Math.min(lateMs, o.maxRaiseStepMs)));
    const raised = target > this.value;
    if (raised) {
      this.value = target;
      this.lastChangeAt = now;
    }
    this.record({ t: now, type: "stall", kind, lateMs: Math.round(lateMs), ms: this.value, quietMs: this.quiet });
    return raised ? this.value : null;
  }

  /** 시간이 지났음을 알린다(통계를 읽을 때마다). 내려야 하면 새 B를, 아니면 null을 돌려준다. */
  tick(now: number): number | null {
    const o = this.opt;
    this.firstAt ??= now;
    const lastStall = this.lastStallAt ?? this.firstAt;
    if (this.quiet !== o.quietMs && now - lastStall >= o.calmResetMs) this.quiet = o.quietMs;
    if (this.value <= o.baseMs) return null;
    const since = Math.max(lastStall, this.lastChangeAt ?? -Infinity);
    if (now - since < this.quiet) return null;
    this.value = Math.max(o.baseMs, this.value - o.lowerStepMs);
    this.lastChangeAt = now;
    this.lastLowerAt = now;
    this.record({ t: now, type: "lower", kind: null, lateMs: null, ms: this.value, quietMs: this.quiet });
    return this.value;
  }

  private record(e: PlayoutBufferEvent): void {
    this.log.push(e);
    if (this.log.length > 100) this.log.shift();
  }
}

/** RTP 시각(90kHz, 32비트)의 차이. 뒤로 간 것(순서 뒤바뀜)이면 null */
function rtpDelta(prev: number, cur: number): number | null {
  const d = (((cur - prev) % 0x1_0000_0000) + 0x1_0000_0000) % 0x1_0000_0000;
  return d >= 0x8000_0000 ? null : d;
}

/** 화면에 나온 비디오 프레임 하나(`requestVideoFrameCallback`의 값) */
export interface FrameMark {
  /** 표시 시각(ms, `expectedDisplayTime`) */
  displayMs: number;
  /** RTP 시각(90kHz, `rtpTimestamp`). 방송자의 캡처 시각에서 만든다 */
  rtp: number;
  /** 프레임을 다 받은 시각(ms, `receiveTime`). 브라우저가 주지 않으면 null */
  receiveMs?: number | null;
}

/**
 * 연속한 두 프레임 사이에서 비디오가 늦은 정도(ms): 표시 간격 − 송신 간격.
 * 정지 화면처럼 송신 간격이 길면 표시 간격도 같이 길어 0 근처가 된다. RTP가 뒤로 갔거나 표시가 거꾸로면 null.
 */
export function videoLateMs(prev: FrameMark, cur: FrameMark): number | null {
  const d = rtpDelta(prev.rtp, cur.rtp);
  const shown = cur.displayMs - prev.displayMs;
  if (d === null || !Number.isFinite(shown) || shown < 0) return null;
  return shown - d / 90;
}

/** 연속한 두 프레임 사이에서 늦게 받은 정도(ms): 받은 간격 − 송신 간격. 받은 시각이 없거나 RTP가 뒤로 갔으면 null */
export function videoArrivalLateMs(prev: FrameMark, cur: FrameMark): number | null {
  const d = rtpDelta(prev.rtp, cur.rtp);
  if (d === null || prev.receiveMs == null || cur.receiveMs == null) return null;
  const got = cur.receiveMs - prev.receiveMs;
  return Number.isFinite(got) ? got - d / 90 : null;
}

/**
 * 버퍼로 막을 수 있는 비디오 끊김의 크기(ms). 화면에 늦게 나온 정도(`videoLateMs`)를 돌려주되, 받은 시각이 있는데 늦게 받은 정도가
 * 그 절반도 안 되면(제때 받았는데 디코딩·그리기가 늦음) null. 네트워크로 늦은 프레임은 버퍼가 남은 여유만큼 덜 늦게 나오므로
 * 늦게 받은 정도가 화면에 늦게 나온 정도보다 크거나 같다.
 */
export function videoStallMs(prev: FrameMark, cur: FrameMark): number | null {
  const late = videoLateMs(prev, cur);
  if (late === null) return null;
  const arrival = videoArrivalLateMs(prev, cur);
  if (arrival !== null && arrival < late * 0.5) return null;
  return late;
}

/**
 * 통계 구간 하나에서 소리가 빈 시간(ms): 은닉 샘플 증가 / 48. 그 구간에 받은 오디오 패킷이 없으면(방송자 쪽 중단·통계 시작)
 * 버퍼와 상관없는 공백이라 null.
 */
export function audioLateMs(concealedDelta: number, packetsDelta: number, sampleRate = 48_000): number | null {
  if (packetsDelta <= 0 || concealedDelta <= 0) return null;
  return concealedDelta / (sampleRate / 1000);
}
