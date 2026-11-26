// 시청 버퍼(D-072, 값 D-094): 비디오·오디오 두 수신기에 같이 거는 최소 재생 지연(`jitterBufferTarget`) B를 정한다.
//
// 150ms에서 시작하고 150ms 밑으로 내리지 않는다(D-094: 처음부터 재전송 한두 번이 들어올 여유를 둔다).
// 재생이 끊기면(패킷이 버퍼보다 늦게 와서 화면이 멈추거나 소리가 비면) 늦은 만큼 B를 올리고
// (한 번에 최대 100ms, 전체 최대 500ms), 10초 동안 끊김이 없으면 15ms(60fps에서 약 1프레임)씩 기본값까지 내린다.
// 내린 뒤 곧 다시 끊기면(되돌이) 다음에 내리기까지 기다리는 시간을 두 배로 늘린다(최대 1분: 상한에 오래 붙어 있지 않게). 오래 조용하면 다시 짧게.
//
// 두 수신기에 같은 B를 거는 까닭: 망 흔들림은 두 트랙에 똑같이 오고, 한쪽만 올리면 브라우저 자체 A/V 동기화가
// 다른 쪽을 천천히 따라 올리는 동안 A/V가 어긋난다. A/V 동기화는 브라우저에만 맡긴다(D-106).
//
// 끊김 신호(호출하는 쪽이 재서 `stall`로 넣는다):
// - 비디오: `requestVideoFrameCallback`의 연속한 두 프레임에서 "표시 간격 − 송신 간격(RTP 시각)"(`videoStallMs`).
//   제때 받았는데 늦게 나온 것(디코딩·그리기 쪽 멈춤, 받은 간격 − 송신 간격이 작음)은 버퍼로 막을 수 없어 세지 않는다.
//   화면이 멈춰 있으면 방송자가 새 프레임을 보내지 않으므로(crates/media video.rs) 표시 간격만으로는 끊김을 알 수 없다.
//   브라우저의 `freezeCount`도 같은 까닭으로 정지 화면을 멈춤으로 셀 수 있어(추정) 쓰지 않는다.
// - 비디오 쪽 두 경우(시험으로 고정): 키프레임 대기는 다음에 나오는 키프레임이 새로 캡처된 것이라 표시 간격과 RTP 간격이 같이 길어
//   late ≈ 0이라 세지 않는다(버퍼로 못 막는 멈춤). 재전송(RTX) 대기는 프레임이 RTT만큼 늦게 다 모여 받은 시각도 늦으므로 late로 센다
//   (B가 RTT보다 크면 막을 수 있는 늦음이다).
// - 오디오: 통계 구간의 은닉 샘플 증가에서 손실 몫을 뺀 것(`audioStall`). 방송자는 소리가 없어도 무음 패킷을 계속 보내므로
//   (crates/media audio) 은닉은 패킷이 늦었거나 빠졌다는 뜻이다. 오디오에는 NACK·FEC가 없고(Opus 10ms CELT, D-080) RED(D-087)는
//   가까운 손실만 되살리므로 못 되살린 패킷은 그대로 은닉이 된다. 버퍼로 손실은 못 막으므로 손실로 생긴 은닉은 빼고,
//   늦게 와서 생긴 은닉만 B를 올린다(D-088). libwebrtc NetEq는 빠진 패킷 자리를 은닉으로 채우고(packetsLost는 seq 빈자리로 센다)
//   늦게 와서 버린 패킷의 자리도 이미 은닉에 들어 있다. 그래서 packetsDiscarded는 보지 않는다(RED는 중복 블록이 버림으로 세어진다).
//   손실 몫을 은닉에서 빼는 것은 근사다(미검증: WebView2에서 손실 1패킷당 은닉이 정확히 10ms인지).
//
// 시청마다 새로 만든다(시청 사이에 기억하지 않는다, D-072). 순수 로직이다(시계·DOM 없음). 시각은 호출하는 쪽의 단조 시계(ms, performance.now)다.

export interface PlayoutBufferOptions {
  /** 내림 바닥, 곧 최솟값이자 처음 값(ms). `maxMs`보다 크면 `maxMs`에서 시작한다 */
  baseMs: number;
  /** 최댓값(ms). 지연 목표는 두지 않는다(D-095). 오디오 지터 버퍼 한도(패킷 200개 = 10ms 패킷으로 2초, NetEq 목표 상한 1500ms)보다 충분히 작게 둔다 */
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
  baseMs: 150,
  maxMs: 500,
  minStallMs: 50,
  maxStallMs: 2000,
  maxRaiseStepMs: 100,
  mergeMs: 2000,
  startupMs: 2000,
  quietMs: 10_000,
  maxQuietMs: 60_000,
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
    this.value = Math.min(this.opt.maxMs, this.opt.baseMs);
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

  /**
   * `now`부터 `ms` 동안 끊김을 보지 않는다. 받는 스트림이 바뀌었을 때(해상도 변경 등), 창이 숨겨졌다 돌아왔을 때 부른다.
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

/** 오디오 통계 구간(이전 읽기와의 차이) */
export interface AudioIntervalDelta {
  /** 구간 은닉 샘플 증가(inbound-rtp concealedSamples) */
  concealedSamples: number;
  /** 구간 수신 패킷 증가 */
  packetsReceived: number;
  /** 구간 packetsLost 증가(오디오만). 늦게 온 패킷 때문에 줄 수 있어 음수는 0으로 본다 */
  packetsLost: number;
}

/** 구간의 소리 빈 시간을 가른 결과(ms) */
export interface AudioStall {
  /** 은닉 전체 */
  concealedMs: number;
  /** 손실로 생긴 몫(버퍼로 못 막는다) */
  lossMs: number;
  /** 늦어서 생긴 몫(B를 올릴 값). 없으면 null */
  lateMs: number | null;
}

/**
 * 오디오 구간에서 손실과 늦음을 가른다(D-088). 받은 패킷이 없거나 은닉이 없으면 null.
 * 늦음 = 은닉 − 손실 몫. packetsDiscarded(늦게 와서 버린 패킷)는 쓰지 않는다: 그 자리는 이미 은닉으로 채워져 은닉에 들어 있어
 * 따로 세면 겹쳐 세고, RED를 켜면 중복 블록이 초당 약 195개씩 버림으로 세어져 B가 300까지 계속 오른다.
 * RED 한계: RED가 되살린 손실도 packetsLost에 들어가(seq 빈자리로 센다) 손실 몫이 실제보다 커지고, 같은 구간의 진짜 늦음을 덜 볼 수 있다.
 * B를 덜 올리는 쪽이라 지연이 늘지 않는 안전한 쪽이다.
 * `PlayoutBuffer.stall(now, lateMs, "audio")`에는 `lateMs`만 넣는다(`minStallMs` 미만은 거기서 무시되므로
 * 은닉 길이가 손실 패킷 수와 조금 안 맞는 오차도 흡수된다). `concealedMs`·`lossMs`는 기록용이다.
 */
export function audioStall(
  d: AudioIntervalDelta,
  opts: { sampleRate?: number; samplesPerPacket?: number } = {},
): AudioStall | null {
  const sampleRate = opts.sampleRate ?? 48_000;
  const samplesPerPacket = opts.samplesPerPacket ?? 480; // Opus 10ms(D-080)
  if (d.packetsReceived <= 0 || d.concealedSamples <= 0) return null;
  const perMs = sampleRate / 1000;
  const packetMs = samplesPerPacket / perMs;
  const concealedMs = d.concealedSamples / perMs;
  const lossMs = Math.max(0, d.packetsLost) * packetMs;
  const late = concealedMs - lossMs;
  return { concealedMs, lossMs, lateMs: late > 0 ? late : null };
}
