// 립싱크 보정: 시청을 시작한 직후 비디오가 오디오보다 앞서는 구간(약 +90~120ms)을 빠르게 없앤다.
//
// 브라우저(libwebrtc)의 A/V 동기화는 비디오 지연을 아주 천천히(약 20초에 걸쳐) 늘린다. 시청 페이지는 두 수신기의
// `jitterBufferTarget`(최소 재생 지연, 바닥)을 시청 버퍼 B(playoutBuffer.ts, 기본 100ms)로 걸어 두는데, 이 값은 브라우저 립싱크가 거는 최소 지연과 "최댓값"으로
// 합쳐진다(`VideoReceiveStream2::UpdatePlayoutDelays`). 그래서 페이지가 getStats의 estimatedPlayoutTimestamp(EPT) 차이를 보며
// 비디오 바닥을 직접 올리고 내린다(적분 제어). 오디오 쪽 바닥은 비디오가 오디오보다 늦을 때만 쓴다.
// (최댓값으로 합쳐진다는 점은 libwebrtc 소스를 읽은 기억에 따른 것이다. 실측에서 바닥을 0으로 내려도 비디오 목표 지연이
// 브라우저 동기화 값(137ms)에 머문 것과 맞는다.)
//
// 이 보정기가 내는 바닥은 B 위에 얹는 차이다(D-072). 페이지가 B를 더해 수신기에 걸고, 넣는 샘플의 목표 지연·지터 버퍼 지연도
// B를 뺀 값이다. B가 바뀌면 `baseChanged`로 잠시 재지 않는다. B가 0이면 예전(D-060)과 똑같이 동작한다.
//
// 지표: avOffsetMs = 비디오 EPT − 오디오 EPT(ms). 양수면 비디오가 앞선다. 두 EPT 모두 같은 송신 시계(방송자의 캡처 시각,
// SR의 NTP 시각)로 환산한 값이라 방송자·시청자 PC의 시계 차이가 빠진다(차이를 구할 때 상쇄).
//
// 불안정 구간에는 움직이지 않는다(2026-11-20 실측 원자료에 따른 규칙):
// 오디오·비디오가 함께 0.2~0.4초 끊기면(은닉 `concealedSamples` 7440~15000 증가, `freezeCount` +1) NetEq가 지연을 크게 올렸다가
// 몇 초에 걸쳐 빼낸다(lan/h264: 오디오 목표 40 → 372~400ms가 약 5초, 이어서 3.5초 동안 지연이 빠짐). 이때의 EPT 차이는 비디오가
// 늦은 게 아니라 오디오 지연이 일시적으로 움직인 결과라, 따라가며 비디오 바닥을 올리면 지연이 빠진 뒤 E가 −109~−210ms로 지나친다.
// 그래서 (1) 끊김 사건 뒤 잠시, (2) 오디오 목표 지연이 평소보다 치솟았거나 지연이 목표보다 크게 밀려 있는 동안에는 샘플을 모으지도,
// 바닥을 바꾸지도 않는다. 오래 이어지는(`maxUnstableMs`) 변화는 새 기준으로 받아들이고, 그때는 한 번에 `maxFollowStepMs`씩만 따라간다
// (바닥을 한 번에 크게 올리면 그만큼 비디오가 멈춘다: +194ms → 프레임 공백 222ms).
//
// 순수 로직이다(시계·DOM 없음). 호출하는 쪽이 샘플을 넣고 돌려받은 바닥을 수신기에 건다.

/**
 * 목표 보정값(ms): EPT 차이를 0이 아니라 이 값으로 맞춘다. 양수면 비디오가 이만큼 앞선 상태를 "맞음"으로 본다.
 * EPT 차이와 실제 출력(스피커·화면)에서 잰 어긋남 사이에 치우침이 있으면 실측해서 여기만 고친다.
 *
 * 0으로 둔 근거(2026-11-20):
 * - 오디오 EPT는 libwebrtc가 NetEq 재생 시각에서 출력 장치 지연(ADM `PlayoutDelay`)을 빼서 구한다(`ChannelReceive::GetPlayoutRtpTimestamp`).
 *   출력 지연이 이미 들어 있다. 비디오 EPT는 렌더러에 넘기는 시각이라 화면 표시 지연만 빠진다. 같은 프레임의 rVFC
 *   `expectedDisplayTime`과 비교하면 4~21ms, 평균 약 10ms였다(하네스 av, 실행 20개).
 * - WebAudio(MediaStreamAudioSourceNode → AudioWorklet)로 잰 차이 W에는 측정 경로 지연이 +31~34ms로 늘 붙는다(재생 경로가 아님).
 *   W=0에 맞추면 E ≈ −23ms, 곧 소리가 30ms 넘게 먼저 나온다. 소리가 먼저인 쪽이 더 잘 보인다(ITU-R BT.1359 감지 한계: 소리 먼저 45ms, 나중 125ms).
 * - E=0이면 실제로는 소리가 10ms 남짓(+모니터 지연) 먼저일 것으로 추정한다(미검증). 감지 한계 안이다.
 *   실제 출력은 소리 없이 잴 수 없어(세션 볼륨 0이면 프로세스 루프백도 0만 받는다) 사람이 귀로 확인한다.
 *   소리가 먼저로 들리면 +10~20으로 올린다.
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
  /** 바닥을 한 번이라도 바꾼 뒤(그리고 `reset()` 전)에 보정하려면 모여야 하는 최소 샘플 수. 튀는 값 하나에 덜 흔들리게 `windowSize`와 같게 둔다. */
  minSamples: number;
  /**
   * 지금 건 보정의 크기를 줄이는 방향(비디오 바닥이 올라가 있는데 비디오가 늦거나, 오디오 바닥이 올라가 있는데 비디오가 앞섬)의 보정에
   * 필요한 최소 샘플 수. 바닥을 낮추는 쪽은 틀려도 손해가 작아(다시 올리면 된다) 하나로 빨리 따라간다. 이 빠른 길은 바닥을 0까지만
   * 줄이고, 남은 차이는 다음 판단(늘리는 쪽 규칙)에 맡긴다. 늘리는 쪽은 `minSamples`개 중앙값으로 튀는 값에 덜 흔들리게 둔다.
   * (실측 2026-11-20: 끊김으로 오디오 지연이 53 → 253ms로 뛰었다 내려올 때, 샘플 3개를 모으느라 6초 동안 A/V가 −110~−152ms였다.)
   */
  reduceMinSamples: number;
  /**
   * 아직 한 번도 바닥을 바꾸지 않았을 때(`reset()` 직후) 필요한 최소 샘플 수. 처음 값은 늘 크고(+65~100ms) 잡음(±1프레임 ≈ 33ms)보다
   * 훨씬 커서 하나로도 안전하다. EPT가 처음 나오는 약 2초에 바로 보정하려고 1로 둔다. `streamChanged()`는 이 상태를 지우지 않는다.
   */
  firstMinSamples: number;
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
  /**
   * 사건(끊김)으로 보는 오디오 은닉 샘플 증가량(직전 샘플 대비, 48kHz 20ms = 960). 정상 실행의 은닉은 시작 직후 480~1920이 늘어나는 정도이고
   * 끊김은 7440~15000이 늘었다. `freezeCount`가 늘어도 사건이다.
   */
  concealMinSamples: number;
  /** 보정기를 처음 부른 뒤 이 시간 안의 은닉 증가는 사건으로 보지 않는다(ms). 시작 직후 은닉은 정상이다(`freezeCount`는 해당 없음). */
  startupMs: number;
  /** 사건이 난 뒤 불안정으로 보는 시간(ms) */
  triggerHoldMs: number;
  /** 오디오 목표 지연이 기준(안정할 때의 중앙값)보다 이만큼 이상 높으면 치솟음으로 본다(ms). 정상 실행의 목표는 20~40ms 흔들린다. */
  spikeMs: number;
  /**
   * 오디오 지연(구간 평균 `jitterBufferDelay`)이 목표 지연보다 이만큼 이상 크면 NetEq가 밀린 것을 빼는 중이라 본다(ms).
   * 정상 실행에서 지연 − 목표는 −40~+24ms였고, 끊김 뒤에는 +100~+240ms였다.
   */
  backlogMs: number;
  /** 오디오 목표 지연의 기준(중앙값)을 구하는 안정 샘플 수 */
  baselineWindow: number;
  /**
   * 불안정이 시작된 뒤 치솟음·밀림이 이만큼 이어지면 새 기준으로 받아들인다(ms). 실측: lan/h264의 밀림은 사건 뒤 9.5초 만에 풀렸다(8초면
   * 빠지는 도중에 받아들여 그 값을 따라간다). 끊김이 아닌 오래 가는 지연 변화는 이 시간이 지나면 따라간다.
   */
  maxUnstableMs: number;
  /** 불안정이 끝난 뒤 이만큼 계속 안정이어야 다시 샘플을 모은다(ms) */
  recoverHoldMs: number;
  /**
   * 첫 보정 뒤 "늘리는 쪽" 보정(비디오 또는 오디오 바닥을 더 올림)이 한 번에 바꾸는 최대량(ms). 첫 보정은 `maxStepMs`까지다.
   * 줄이는 쪽은 제한하지 않는다(바닥을 낮추는 쪽은 틀려도 손해가 작다).
   */
  maxFollowStepMs: number;
  /** 오디오 목표 지연이 직전 샘플보다 이만큼 이상 바뀌면 모은 샘플을 버린다(ms). 대기는 걸지 않는다. */
  audioTargetJumpMs: number;
}

export const LIPSYNC_DEFAULTS: LipSyncOptions = {
  targetOffsetMs: LIPSYNC_TARGET_OFFSET_MS,
  // 8: 정상 상태의 EPT 차이는 ±2ms 안에서만 흔들리고, 첫 보정 뒤에 +9~13ms가 남는 일이 많아 12는 그 잔차를 못 잡았다(실측 2026-11-20).
  deadbandMs: 8,
  maxStepMs: 200,
  maxAdjustMs: 600,
  outlierMs: 2000,
  windowSize: 3,
  minSamples: 3,
  reduceMinSamples: 1,
  firstMinSamples: 1,
  gain: 1,
  slewMsPerSec: 100,
  settleExtraMs: 500,
  fastPhaseMs: 15_000,
  fastIntervalMs: 500,
  slowIntervalMs: 1000,
  maxGapMs: 5000,
  streamChangeHoldMs: 1000,
  concealMinSamples: 960,
  startupMs: 2000,
  triggerHoldMs: 2000,
  spikeMs: 60,
  backlogMs: 60,
  baselineWindow: 10,
  maxUnstableMs: 12_000,
  recoverHoldMs: 1500,
  maxFollowStepMs: 50,
  audioTargetJumpMs: 30,
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
  /** 직전 구간의 평균 목표 지연(ms)에서 시청 버퍼 B를 뺀 값. 지금 바닥이 실제로 효과가 있는 크기인지 가늠하는 데 쓴다 */
  videoTargetMs: number | null;
  audioTargetMs: number | null;
  /** 직전 구간의 평균 오디오 지터 버퍼 지연(ms)에서 B를 뺀 값. 목표보다 크게 크면 NetEq가 밀린 것을 빼는 중이다. 없으면 null */
  audioJitterMs: number | null;
  /** 오디오 은닉 샘플 누적 수(`concealedSamples`) */
  audioConcealedSamples: number;
  /** 비디오 멈춤 누적 수(`freezeCount`) */
  freezeCount: number;
}

/** 보정기가 지금 샘플을 모으는가: 안정 / 불안정(움직이지 않음) / 회복 중(불안정이 끝난 뒤 `recoverHoldMs` 동안 계속 안정인지 보는 중) */
export type LipSyncState = "stable" | "unstable" | "recovering";
/** 불안정이 된 이유 */
export type LipSyncReason = "conceal" | "freeze" | "audio-spike" | "audio-backlog";

/** 개발용 읽기 전용 상태 */
export interface LipSyncDebug {
  state: LipSyncState;
  /** 안정이면 null, 불안정·회복 중이면 마지막 이유 */
  reason: LipSyncReason | null;
  /** 안정할 때의 오디오 목표 지연 중앙값(ms). 아직 없으면 null */
  audioBaseline: number | null;
}

/** 수신기에 걸 최소 재생 지연(ms, 0 이상). 시청 버퍼 B 위에 얹는 차이다(페이지가 B를 더해 건다). */
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
  /** `reset()` 뒤 바닥을 한 번이라도 바꿨는가. 바꿨으면 `minSamples`, 아니면 `firstMinSamples`로 판단한다. */
  private adjusted = false;
  /** 이 시각 전에는 재지 않는다(수신기가 따라오는 중) */
  private holdUntil = -Infinity;
  private lastSampleAt: number | null = null;
  private lastFrames: number | null = null;
  private lastAudioPackets: number | null = null;
  private startedAt: number | null = null;

  // --- 불안정 판정 상태 ---
  private state: LipSyncState = "stable";
  private reason: LipSyncReason | null = null;
  /** 보정기를 처음 부른 시각(`startupMs` 기준) */
  private firstAt: number | null = null;
  private prevConcealed: number | null = null;
  private prevFreeze: number | null = null;
  private prevAudioTarget: number | null = null;
  /** 이 시각 전에는 사건 뒤 불안정이다 */
  private triggerUntil = -Infinity;
  private triggerReason: LipSyncReason | null = null;
  /** 지금 불안정 구간(회복 중 포함)이 시작된 시각. 안정이면 null */
  private unstableSince: number | null = null;
  /** 불안정이 끝나 회복을 세기 시작한 시각 */
  private stableSince = 0;
  /** 안정 샘플의 오디오 목표 지연(최근 `baselineWindow`개) */
  private baselineHistory: number[] = [];
  /** 새 기준으로 받아들인 뒤, 밀림 조건이 한 번 풀릴 때까지 밀림을 보지 않는다 */
  private ignoreBacklog = false;

  constructor(options: Partial<LipSyncOptions> = {}) {
    this.opt = { ...LIPSYNC_DEFAULTS, ...options };
  }

  /** 지금 건 보정(ms). 양수면 비디오를 그만큼 늦췄다는 뜻이고, 음수면 오디오를 늦춘 것이다. */
  get adjustMs(): number {
    return this.adjust;
  }

  /** 개발용: 지금 상태, 이유, 오디오 목표 지연 기준 */
  get debug(): LipSyncDebug {
    return { state: this.state, reason: this.reason, audioBaseline: this.baseline() };
  }

  private baseline(): number | null {
    return this.baselineHistory.length > 0 ? median(this.baselineHistory) : null;
  }

  get floors(): LipSyncFloors {
    return { videoMs: Math.max(this.adjust, 0), audioMs: Math.max(-this.adjust, 0) };
  }

  /** 다음 통계까지의 간격(ms): 처음 15초는 빠르게, 그 뒤는 1초 */
  nextIntervalMs(now: number): number {
    this.startedAt ??= now;
    return now - this.startedAt < this.opt.fastPhaseMs ? this.opt.fastIntervalMs : this.opt.slowIntervalMs;
  }

  /** 코덱·해상도가 바뀌었다: 모은 샘플을 버리고 잠시 재지 않는다. 보정값과 "바꿨음" 상태는 그대로 둔다(그 뒤에도 `minSamples`). */
  streamChanged(now: number): void {
    this.samples = [];
    this.holdUntil = Math.max(this.holdUntil, now + this.opt.streamChangeHoldMs);
    this.lastFrames = null;
  }

  /**
   * 두 수신기에 같이 거는 시청 버퍼 B(playoutBuffer.ts)가 `deltaMs`만큼 바뀌었다. 보정값은 B 위의 차이라 그대로 두고,
   * 수신기가 새 B를 따라오는 동안(두 트랙이 같은 빠르기로 따라온다는 보장이 없다) 모은 샘플을 버리고 재지 않는다.
   */
  baseChanged(now: number, deltaMs: number): void {
    if (deltaMs === 0) return;
    this.samples = [];
    this.holdUntil = Math.max(
      this.holdUntil,
      now + (Math.abs(deltaMs) / this.opt.slewMsPerSec) * 1000 + this.opt.settleExtraMs,
    );
  }

  /** 새 연결처럼 처음부터 */
  reset(): void {
    this.adjust = 0;
    this.adjusted = false;
    this.samples = [];
    this.holdUntil = -Infinity;
    this.lastSampleAt = null;
    this.lastFrames = null;
    this.lastAudioPackets = null;
    this.startedAt = null;
    this.state = "stable";
    this.reason = null;
    this.firstAt = null;
    this.prevConcealed = null;
    this.prevFreeze = null;
    this.prevAudioTarget = null;
    this.triggerUntil = -Infinity;
    this.triggerReason = null;
    this.unstableSince = null;
    this.stableSince = 0;
    this.baselineHistory = [];
    this.ignoreBacklog = false;
  }

  /**
   * 안정 여부를 갱신한다(샘플마다 부른다). 안정이면 true.
   * - 사건: 은닉이 `concealMinSamples` 이상 늘었거나(시작 `startupMs` 안은 제외) `freezeCount`가 늘면 `triggerHoldMs` 동안 불안정.
   * - 치솟음: 오디오 목표 ≥ 기준 + `spikeMs`, 밀림: 오디오 지연 − 목표 ≥ `backlogMs`. 해소될 때까지 불안정.
   * - 치솟음·밀림이 불안정 시작 뒤 `maxUnstableMs`가 지나도 이어지면 새 기준으로 받아들인다.
   * - 불안정이 끝난 뒤 `recoverHoldMs` 동안 계속 안정이어야 안정으로 돌아온다.
   */
  private updateStability(s: LipSyncSample): boolean {
    const o = this.opt;
    const now = s.now;
    this.firstAt ??= now;
    const startup = now - this.firstAt < o.startupMs;

    const concealDelta = this.prevConcealed === null ? 0 : s.audioConcealedSamples - this.prevConcealed;
    const freezeDelta = this.prevFreeze === null ? 0 : s.freezeCount - this.prevFreeze;
    this.prevConcealed = s.audioConcealedSamples;
    this.prevFreeze = s.freezeCount;
    let event: LipSyncReason | null = null;
    if (concealDelta >= o.concealMinSamples && !startup) event = "conceal";
    else if (freezeDelta > 0) event = "freeze";
    if (event !== null) {
      this.triggerUntil = now + o.triggerHoldMs;
      this.triggerReason = event;
    }

    const aT = s.audioTargetMs;
    const aJ = s.audioJitterMs;
    const base = this.baseline();
    const backlogHit = aT !== null && aJ !== null && aJ - aT >= o.backlogMs;
    if (!backlogHit) this.ignoreBacklog = false;
    let elevated: LipSyncReason | null = null;
    if (aT !== null && base !== null && aT >= base + o.spikeMs) elevated = "audio-spike";
    else if (backlogHit && !this.ignoreBacklog) elevated = "audio-backlog";

    const triggered = now < this.triggerUntil;
    if ((triggered || elevated !== null) && this.unstableSince === null) this.unstableSince = now;
    if (elevated !== null && this.unstableSince !== null && now - this.unstableSince >= o.maxUnstableMs) {
      // 오래 이어지는 변화: 새 기준으로 받아들인다. 받아들인 뒤 다시 `maxUnstableMs`가 지나야 또 받아들인다.
      if (aT !== null) this.baselineHistory = [aT];
      this.ignoreBacklog = true;
      this.unstableSince = now;
      elevated = null;
    }

    if (triggered || elevated !== null) {
      this.state = "unstable";
      this.reason = triggered ? this.triggerReason : elevated;
    } else if (this.state === "unstable") {
      this.state = "recovering";
      this.stableSince = now;
    }
    if (this.state === "recovering" && now - this.stableSince >= o.recoverHoldMs) {
      this.state = "stable";
      this.reason = null;
      this.unstableSince = null;
    }

    // 안정 샘플만 기준에 넣는다(치솟은 값이 기준이 되면 안 된다).
    if (this.state === "stable" && aT !== null) {
      this.baselineHistory.push(aT);
      if (this.baselineHistory.length > o.baselineWindow) this.baselineHistory.shift();
    }
    // 샘플 위생: 오디오 지연이 움직이는 중에 모은 값을 섞지 않는다(대기는 걸지 않는다).
    if (aT !== null) {
      if (this.prevAudioTarget !== null && Math.abs(aT - this.prevAudioTarget) >= o.audioTargetJumpMs) {
        this.samples = [];
      }
      this.prevAudioTarget = aT;
    }
    return this.state === "stable";
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

    // 불안정(사건 뒤·오디오 지연 치솟음·회복 중)에는 샘플을 모으지 않고 바닥도 바꾸지 않는다.
    if (!this.updateStability(s)) {
      this.samples = [];
      return null;
    }

    const raw = s.avOffsetMs;
    if (raw === null || !Number.isFinite(raw) || Math.abs(raw) > o.outlierMs) return null;
    if (!framesAdvanced || !audioAdvanced) return null;
    if (s.now < this.holdUntil) return null;

    // 이후는 모두 "목표 보정값에서 벗어난 정도"로 계산한다.
    this.samples.push(raw - o.targetOffsetMs);
    if (this.samples.length > o.windowSize) this.samples.shift();

    const m = median(this.samples);
    // 모인 샘플의 중앙값이 지금 건 보정을 줄이는 방향이면 빠른 길(`reduceMinSamples`), 아니면 기존 규칙.
    const reducing = (this.adjust > 0 && m < 0) || (this.adjust < 0 && m > 0);
    const need = reducing ? o.reduceMinSamples : this.adjusted ? o.minSamples : o.firstMinSamples;
    if (this.samples.length < need) return null;
    if (Math.abs(m) <= o.deadbandMs) return null;

    // 첫 보정은 maxStepMs까지, 그 뒤 늘리는 쪽은 maxFollowStepMs까지. 줄이는 쪽은 그대로 maxStepMs.
    const limit = this.adjusted && !reducing ? Math.min(o.maxStepMs, o.maxFollowStepMs) : o.maxStepMs;
    const step = Math.max(-limit, Math.min(limit, m * o.gain));
    // 바닥은 "최소 지연"이라, 자연 지연(목표 지연 통계)보다 낮은 바닥은 효과가 없다.
    // 같은 방향으로 올릴 때는 실제 지연(목표 지연)에서부터 더한다.
    let next: number;
    if (reducing) {
      // 줄이는 길은 0을 넘지 않는다. 반대쪽 바닥은 새로 올리지 않고, 남은 차이는 다음 판단에 맡긴다.
      next = this.adjust > 0 ? Math.max(this.adjust + step, 0) : Math.min(this.adjust + step, 0);
    } else if (step > 0) {
      next = Math.max(this.adjust, s.videoTargetMs ?? 0) + step;
    } else {
      next = -(Math.max(-this.adjust, s.audioTargetMs ?? 0) - step);
    }
    next = Math.round(Math.max(-o.maxAdjustMs, Math.min(o.maxAdjustMs, next)));
    if (next === this.adjust) return null;

    this.adjust = next;
    this.adjusted = true;
    this.samples = [];
    this.holdUntil = s.now + (Math.abs(step) / o.slewMsPerSec) * 1000 + o.settleExtraMs;
    return this.floors;
  }
}
