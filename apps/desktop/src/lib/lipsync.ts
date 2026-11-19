// 립싱크 보정: 시청을 시작한 직후 비디오가 오디오보다 앞서는 구간(약 +90~120ms)을 빠르게 없앤다.
//
// 브라우저(libwebrtc)의 A/V 동기화는 비디오 지연을 아주 천천히(약 20초에 걸쳐) 늘린다. 시청 페이지는 두 수신기의
// `jitterBufferTarget`(최소 재생 지연, 바닥)을 0으로 걸어 두는데, 이 값은 브라우저 립싱크가 거는 최소 지연과 "최댓값"으로
// 합쳐진다(`VideoReceiveStream2::UpdatePlayoutDelays`). 그래서 페이지가 getStats의 estimatedPlayoutTimestamp(EPT) 차이를 보며
// 비디오 바닥을 직접 올리고 내린다(적분 제어). 오디오 쪽 바닥은 비디오가 오디오보다 늦을 때만 쓴다.
// (최댓값으로 합쳐진다는 점은 libwebrtc 소스를 읽은 기억에 따른 것으로 미검증이다. 실측은 팀장이 한다.)
//
// 지표: avOffsetMs = 비디오 EPT − 오디오 EPT(ms). 양수면 비디오가 앞선다. 두 EPT 모두 같은 송신 시계(방송자의 캡처 시각,
// SR의 NTP 시각)로 환산한 값이라 방송자·시청자 PC의 시계 차이가 빠진다(차이를 구할 때 상쇄).
//
// 순수 로직이다(시계·DOM 없음). 호출하는 쪽이 샘플을 넣고 돌려받은 바닥을 수신기에 건다.

/**
 * 목표 보정값(ms): EPT 차이를 0이 아니라 이 값으로 맞춘다. 양수면 비디오가 이만큼 앞선 상태를 "맞음"으로 본다.
 * EPT 차이와 실제 출력(스피커·화면)에서 잰 어긋남 사이에 치우침이 있으면 실측해서 여기만 고친다.
 */
export const LIPSYNC_TARGET_OFFSET_MS = 0;

export interface LipSyncOptions {
  /** 목표 보정값(ms). 기본은 `LIPSYNC_TARGET_OFFSET_MS` */
  targetOffsetMs: number;
  /** 이 이하의 차이는 무시한다(ms). 작은 흔들림에 수신기를 건드리지 않는다. */
  deadbandMs: number;
  /** 한 번에 바꾸는 최대 보정량(ms) */
  maxStepMs: number;
  /** 바닥의 최대 크기(ms). 비정상 값으로 지연이 끝없이 커지지 않게 한다. */
  maxAdjustMs: number;
  /** 이보다 큰 |차이|는 측정 오류로 보고 버린다(ms) */
  outlierMs: number;
  /** 중앙값을 구하는 최근 샘플 수 */
  windowSize: number;
  /** 보정하려면 모여야 하는 최소 샘플 수 */
  minSamples: number;
  /** 보정량 중 이만큼만 반영한다(1이면 측정한 차이 전부) */
  gain: number;
  /** 브라우저가 비디오 지연을 바꾸는 속도(ms/초). 한 번 움직인 뒤 따라올 때까지 기다리는 시간을 정한다. */
  slewMsPerSec: number;
  /** 움직인 뒤 따라올 시간에 더하는 여유(ms) */
  settleExtraMs: number;
  /** 빠르게 재는 처음 구간(ms)과 그때·그 뒤의 통계 간격(ms) */
  fastPhaseMs: number;
  fastIntervalMs: number;
  slowIntervalMs: number;
  /** 샘플 사이가 이보다 길면(탭이 멈췄다 깨어남 등) 모아 둔 샘플을 버린다(ms) */
  maxGapMs: number;
  /** 코덱·해상도가 바뀐 뒤 잠시 재지 않는 시간(ms) */
  streamChangeHoldMs: number;
}

export const LIPSYNC_DEFAULTS: LipSyncOptions = {
  targetOffsetMs: LIPSYNC_TARGET_OFFSET_MS,
  deadbandMs: 12,
  maxStepMs: 200,
  maxAdjustMs: 600,
  outlierMs: 2000,
  windowSize: 3,
  minSamples: 2,
  gain: 1,
  slewMsPerSec: 100,
  settleExtraMs: 500,
  fastPhaseMs: 15_000,
  fastIntervalMs: 500,
  slowIntervalMs: 1000,
  maxGapMs: 5000,
  streamChangeHoldMs: 1000,
};

export interface LipSyncSample {
  /** 측정 시각(ms, 단조 시계) */
  now: number;
  /** 비디오 EPT − 오디오 EPT(ms). 양수면 비디오가 앞선다. 오디오·비디오 EPT가 없으면 null */
  avOffsetMs: number | null;
  /** 비디오 디코드 프레임 누적 수. 늘지 않은 구간은 EPT가 낡았으니 무시한다 */
  framesDecoded: number;
  /** 오디오 수신 패킷 누적 수. 늘지 않은 구간은 무시한다 */
  audioPackets: number;
  /** 직전 구간의 평균 목표 지연(ms). 지금 바닥이 실제로 효과가 있는 크기인지 가늠하는 데 쓴다 */
  videoTargetMs: number | null;
  audioTargetMs: number | null;
}

/** 수신기에 걸 최소 재생 지연(ms, 0 이상) */
export interface LipSyncFloors {
  videoMs: number;
  audioMs: number;
}

function median(values: number[]): number {
  const s = [...values].sort((a, b) => a - b);
  const mid = s.length >> 1;
  return s.length % 2 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
}

export class LipSync {
  private readonly opt: LipSyncOptions;
  /** 지금 건 보정(ms). 양수면 비디오 바닥, 음수면 오디오 바닥(크기) */
  private adjust = 0;
  private samples: number[] = [];
  /** 이 시각 전에는 재지 않는다(수신기가 따라오는 중) */
  private holdUntil = -Infinity;
  private lastSampleAt: number | null = null;
  private lastFrames: number | null = null;
  private lastAudioPackets: number | null = null;
  private startedAt: number | null = null;

  constructor(options: Partial<LipSyncOptions> = {}) {
    this.opt = { ...LIPSYNC_DEFAULTS, ...options };
  }

  /** 지금 건 보정(ms). 양수면 비디오를 그만큼 늦췄다는 뜻이고, 음수면 오디오를 늦춘 것이다. */
  get adjustMs(): number {
    return this.adjust;
  }

  get floors(): LipSyncFloors {
    return { videoMs: Math.max(this.adjust, 0), audioMs: Math.max(-this.adjust, 0) };
  }

  /** 다음 통계까지의 간격(ms): 처음 15초는 빠르게, 그 뒤는 1초 */
  nextIntervalMs(now: number): number {
    this.startedAt ??= now;
    return now - this.startedAt < this.opt.fastPhaseMs ? this.opt.fastIntervalMs : this.opt.slowIntervalMs;
  }

  /** 코덱·해상도가 바뀌었다: 모은 샘플을 버리고 잠시 재지 않는다. 보정값은 그대로 둔다. */
  streamChanged(now: number): void {
    this.samples = [];
    this.holdUntil = Math.max(this.holdUntil, now + this.opt.streamChangeHoldMs);
    this.lastFrames = null;
  }

  /** 새 연결처럼 처음부터 */
  reset(): void {
    this.adjust = 0;
    this.samples = [];
    this.holdUntil = -Infinity;
    this.lastSampleAt = null;
    this.lastFrames = null;
    this.lastAudioPackets = null;
    this.startedAt = null;
  }

  /**
   * 샘플 하나를 넣는다. 바닥을 바꿔야 하면 새 바닥을, 아니면 null을 돌려준다.
   * 통계를 읽을 때마다 부른다.
   */
  update(s: LipSyncSample): LipSyncFloors | null {
    const o = this.opt;
    const framesAdvanced = this.lastFrames !== null && s.framesDecoded > this.lastFrames;
    const audioAdvanced = this.lastAudioPackets !== null && s.audioPackets > this.lastAudioPackets;
    this.lastFrames = s.framesDecoded;
    this.lastAudioPackets = s.audioPackets;
    const gap = this.lastSampleAt === null ? 0 : s.now - this.lastSampleAt;
    this.lastSampleAt = s.now;
    if (gap > o.maxGapMs) this.samples = [];

    const raw = s.avOffsetMs;
    if (raw === null || !Number.isFinite(raw) || Math.abs(raw) > o.outlierMs) return null;
    if (!framesAdvanced || !audioAdvanced) return null;
    if (s.now < this.holdUntil) return null;

    // 이후는 모두 "목표 보정값에서 벗어난 정도"로 계산한다.
    this.samples.push(raw - o.targetOffsetMs);
    if (this.samples.length > o.windowSize) this.samples.shift();
    if (this.samples.length < o.minSamples) return null;

    const m = median(this.samples);
    if (Math.abs(m) <= o.deadbandMs) return null;

    const step = Math.max(-o.maxStepMs, Math.min(o.maxStepMs, m * o.gain));
    // 바닥은 "최소 지연"이라, 자연 지연(목표 지연 통계)보다 낮은 바닥은 효과가 없다.
    // 같은 방향으로 올릴 때는 실제 지연(목표 지연)에서부터 더한다.
    let next: number;
    if (step > 0 && this.adjust >= 0) {
      next = Math.max(this.adjust, s.videoTargetMs ?? 0) + step;
    } else if (step < 0 && this.adjust <= 0) {
      next = -(Math.max(-this.adjust, s.audioTargetMs ?? 0) - step);
    } else {
      next = this.adjust + step;
    }
    next = Math.round(Math.max(-o.maxAdjustMs, Math.min(o.maxAdjustMs, next)));
    if (next === this.adjust) return null;

    this.adjust = next;
    this.samples = [];
    this.holdUntil = s.now + (Math.abs(step) / o.slewMsPerSec) * 1000 + o.settleExtraMs;
    return this.floors;
  }
}
