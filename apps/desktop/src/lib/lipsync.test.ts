import { describe, expect, it } from "vitest";
import { LIPSYNC_DEFAULTS, LIPSYNC_TARGET_OFFSET_MS, LipSync, type LipSyncFloors, type LipSyncSample } from "./lipsync";
import { LIPSYNC_FIXTURE_LAN_H264, LIPSYNC_FIXTURE_SERVER_VP8, type LipSyncFixtureRow } from "./lipsync.fixtures";

/** 샘플 만들기. `concealed`·`freezes`는 누적 값이라 바꾸면 그 뒤 샘플에 계속 실린다. */
interface Feeder {
  (now: number, avOffsetMs: number | null, over?: Partial<LipSyncSample>): LipSyncSample;
  concealed: number;
  freezes: number;
}

/**
 * 샘플을 만든다. 프레임·오디오 패킷 수는 호출마다 늘어난다(멈춘 구간을 시험할 때만 고정).
 * 오디오 은닉·멈춤은 기본이 "변화 없음"(0), `audioJitterMs`는 null(밀림 없음)이다.
 */
function feed(): Feeder {
  let frames = 0;
  let packets = 0;
  const f = ((now: number, avOffsetMs: number | null, over: Partial<LipSyncSample> = {}): LipSyncSample => {
    frames += 15;
    packets += 25;
    return {
      now,
      avOffsetMs,
      framesDecoded: frames,
      audioPackets: packets,
      videoTargetMs: null,
      audioTargetMs: null,
      audioJitterMs: null,
      audioConcealedSamples: f.concealed,
      freezeCount: f.freezes,
      ...over,
    };
  }) as Feeder;
  f.concealed = 0;
  f.freezes = 0;
  return f;
}

describe("LipSync 시청 버퍼 B(D-072)", () => {
  it("baseChanged: 보정값은 두고, 따라오는 동안(|Δ|/100ms/초 + 0.5초) 재지 않는다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    expect(ls.update(s(500, 100))).toEqual({ videoMs: 100, audioMs: 0 }); // 자체 대기: 2000까지
    ls.baseChanged(2500, 200); // 2500 + 2000 + 500 = 5000까지
    for (const t of [3000, 3500, 4000, 4500]) expect(ls.update(s(t, 60))).toBeNull();
    expect(ls.adjustMs).toBe(100);
    // 대기가 끝나면 샘플 3개(이미 한 번 바꿨으므로 minSamples)를 모아 다시 보정한다
    expect(ls.update(s(5000, 60))).toBeNull();
    expect(ls.update(s(5500, 60))).toBeNull();
    expect(ls.update(s(6000, 60))).toEqual({ videoMs: 150, audioMs: 0 });
  });

  it("baseChanged(0)은 아무것도 하지 않는다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.baseChanged(200, 0);
    expect(ls.update(s(500, 100))).toEqual({ videoMs: 100, audioMs: 0 });
  });
});

describe("LipSync 기본", () => {
  it("처음엔 바닥이 0이고 아무것도 하지 않는다", () => {
    const ls = new LipSync();
    expect(ls.adjustMs).toBe(0);
    expect(ls.floors).toEqual({ videoMs: 0, audioMs: 0 });
  });

  it("첫 호출은 프레임 증가 기준만 잡고, 다음 유효 샘플 하나로 바로 보정한다", () => {
    const ls = new LipSync();
    const s = feed();
    expect(ls.update(s(0, 100))).toBeNull(); // 직전 프레임 수가 없어 버린다
    const out = ls.update(s(500, 100));
    expect(out).toEqual({ videoMs: 100, audioMs: 0 });
    expect(ls.adjustMs).toBe(100);
  });

  it("기본값: 첫 보정은 샘플 1개, 이후는 windowSize와 같은 3개", () => {
    expect(LIPSYNC_DEFAULTS.firstMinSamples).toBe(1);
    expect(LIPSYNC_DEFAULTS.minSamples).toBe(3);
    expect(LIPSYNC_DEFAULTS.windowSize).toBe(3);
  });

  it("기본값: 줄이는 방향은 샘플 1개, 데드밴드 8", () => {
    expect(LIPSYNC_DEFAULTS.reduceMinSamples).toBe(1);
    expect(LIPSYNC_DEFAULTS.deadbandMs).toBe(8);
  });

  it("비디오가 앞서면(양수) 비디오 바닥을 올리고, 늦으면(음수) 오디오 바닥을 올린다", () => {
    const a = new LipSync();
    const sa = feed();
    a.update(sa(0, 60));
    expect(a.update(sa(500, 60))).toEqual({ videoMs: 60, audioMs: 0 });

    const b = new LipSync();
    const sb = feed();
    b.update(sb(0, -60));
    expect(b.update(sb(500, -60))).toEqual({ videoMs: 0, audioMs: 60 });
    expect(b.adjustMs).toBe(-60);
  });
});

describe("데드밴드·잡음", () => {
  it("데드밴드 이하의 차이는 무시한다", () => {
    const ls = new LipSync();
    const s = feed();
    for (let t = 0; t < 10_000; t += 500) {
      expect(ls.update(s(t, LIPSYNC_DEFAULTS.deadbandMs))).toBeNull();
    }
    expect(ls.adjustMs).toBe(0);
  });

  it("프레임 간격만큼 흔들려도 중앙값으로 보정량을 정한다", () => {
    const ls = new LipSync({ windowSize: 3, firstMinSamples: 3 }); // 첫 보정도 3개로 시험
    const s = feed();
    ls.update(s(0, 0)); // 기준
    ls.update(s(500, 90));
    ls.update(s(1000, 160)); // 튀는 값
    const out = ls.update(s(1500, 95));
    expect(out).toEqual({ videoMs: 95, audioMs: 0 }); // 중앙값 95
  });

  it("흔들림이 데드밴드 안이면 한 번도 움직이지 않는다", () => {
    const ls = new LipSync();
    const s = feed();
    let moved = 0;
    for (let i = 0; i < 40; i++) {
      if (ls.update(s(i * 1000, i % 2 === 0 ? 8 : -8))) moved++;
    }
    expect(moved).toBe(0);
  });
});

describe("이상한 값", () => {
  it("EPT가 없거나(null) 비정상이면 무시한다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    expect(ls.update(s(500, null))).toBeNull();
    expect(ls.update(s(1000, Number.NaN))).toBeNull();
    expect(ls.update(s(1500, 2500))).toBeNull(); // |값| > 2초
    expect(ls.update(s(2000, -2500))).toBeNull();
    expect(ls.adjustMs).toBe(0);
  });

  it("프레임이 늘지 않은 구간은 무시한다", () => {
    const ls = new LipSync();
    const s = feed();
    const a = s(0, 100);
    ls.update(a); // 기준
    // 프레임이 늘지 않음
    expect(ls.update({ ...s(500, 100), framesDecoded: a.framesDecoded })).toBeNull();
    expect(ls.update({ ...s(1000, 100), framesDecoded: a.framesDecoded })).toBeNull();
    expect(ls.adjustMs).toBe(0);
    // 프레임이 다시 늘면 첫 유효 샘플 하나로 보정한다
    expect(ls.update(s(1500, 100))).toEqual({ videoMs: 100, audioMs: 0 });
  });

  it("오디오 패킷이 늘지 않은 구간도 무시한다(오디오가 멈추면 EPT 차이가 벌어진다)", () => {
    const ls = new LipSync();
    const s = feed();
    const a = s(0, 100);
    ls.update(a); // 기준
    expect(ls.update({ ...s(500, 100), audioPackets: a.audioPackets })).toBeNull();
    expect(ls.update({ ...s(1000, 100), audioPackets: a.audioPackets })).toBeNull();
    expect(ls.adjustMs).toBe(0);
    expect(ls.update(s(1500, 100))).toEqual({ videoMs: 100, audioMs: 0 });
  });

  it("샘플 사이가 너무 길면(멈췄다 깨어남) 모아 둔 샘플을 버린다", () => {
    const ls = new LipSync({ firstMinSamples: 2 }); // 버렸는지 보이게 첫 보정도 2개로 시험
    const s = feed();
    ls.update(s(0, 100)); // 기준
    expect(ls.update(s(500, 100))).toBeNull(); // 정상 샘플 하나
    // 한참 뒤 깨어난 첫 샘플은 옛 샘플과 섞이지 않고 혼자라 아직 움직이지 않는다
    expect(ls.update(s(60_000, 100))).toBeNull();
    expect(ls.update(s(60_500, 100))).toEqual({ videoMs: 100, audioMs: 0 });
  });
});

describe("최대 변경량·상한", () => {
  it("한 번에 maxStep까지만 바꾼다", () => {
    const ls = new LipSync({ maxStepMs: 200 });
    const s = feed();
    ls.update(s(0, 900));
    expect(ls.update(s(500, 900))).toEqual({ videoMs: 200, audioMs: 0 });
  });

  it("바닥은 maxAdjust를 넘지 않는다", () => {
    const ls = new LipSync({ maxStepMs: 400, maxAdjustMs: 500 });
    const s = feed();
    let t = 0;
    ls.update(s(t, 900));
    let last = 0;
    for (let i = 0; i < 40; i++) {
      t += 500;
      const out = ls.update(s(t, 900)); // 보정이 안 먹히는 상황을 흉내
      if (out) last = out.videoMs;
    }
    expect(last).toBe(500);
    expect(ls.adjustMs).toBe(500);
    // 상한에 닿은 뒤에는 더 움직이지 않는다
    t += 5000;
    ls.update(s(t, 900));
    expect(ls.update(s(t + 500, 900))).toBeNull();
  });
});

describe("시간 간격(수신기가 따라올 때까지 기다린다)", () => {
  it("움직인 뒤 |변경량|/100ms초 + 0.5초 동안은 재지 않는다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    expect(ls.update(s(500, 100))).toEqual({ videoMs: 100, audioMs: 0 });
    // 100ms 변경 → 1.0초 + 0.5초 = 1.5초(2000까지) 동안 무시
    for (const t of [1000, 1500, 1900]) expect(ls.update(s(t, 100))).toBeNull();
    // 무시한 샘플은 모으지도 않는다: 2000 이후 세 샘플이 모여야 다시 움직인다
    // (첫 보정 뒤 늘리는 쪽은 한 번에 maxFollowStepMs 50까지라 차이 100이어도 +50 → 150. 이전에는 200이었다.)
    expect(ls.update(s(2000, 100))).toBeNull();
    expect(ls.update(s(2500, 100))).toBeNull();
    expect(ls.update(s(3000, 100))).toEqual({ videoMs: 150, audioMs: 0 });
  });

  it("작은 변경은 짧게 기다린다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 30));
    expect(ls.update(s(500, 30))).toEqual({ videoMs: 30, audioMs: 0 });
    // 30ms → 0.3초 + 0.5초 = 0.8초(1300까지)
    expect(ls.update(s(1000, 30))).toBeNull();
    expect(ls.update(s(1300, 30))).toBeNull(); // 첫 샘플
    expect(ls.update(s(1800, 30))).toBeNull();
    expect(ls.update(s(2300, 30))).toEqual({ videoMs: 60, audioMs: 0 });
  });

  it("통계 간격: 처음 15초는 500ms, 그 뒤 1초", () => {
    const ls = new LipSync();
    expect(ls.nextIntervalMs(1000)).toBe(500);
    expect(ls.nextIntervalMs(10_000)).toBe(500);
    // 시작(1000)부터 15000ms가 지나는 16000부터 느린 간격
    expect(ls.nextIntervalMs(15_999)).toBe(500);
    expect(ls.nextIntervalMs(16_000)).toBe(1000);
    expect(ls.nextIntervalMs(60_000)).toBe(1000);
  });
});

describe("수렴", () => {
  /** 모의 브라우저: 비디오 지연이 바닥/목표를 초당 slew만큼 따라간다. 오프셋 = 오디오 지연 − 비디오 지연 */
  function simulate(opts: { audioDelay: number; videoDelay: number; noise?: number; secs: number }) {
    const ls = new LipSync();
    const s = feed();
    let video = opts.videoDelay;
    const audio = opts.audioDelay;
    let floorV = 0;
    const log: { t: number; offset: number }[] = [];
    const dt = 100;
    let seed = 1;
    // 재현되는 [-1, 1] 난수(선형 합동 생성기, 32비트 정수 곱)
    const rnd = () => ((seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff) / 0x7fffffff) * 2 - 1;
    let nextSample = 0;
    for (let t = 0; t <= opts.secs * 1000; t += dt) {
      const wanted = Math.max(floorV, opts.videoDelay);
      const step = (100 * dt) / 1000;
      video += Math.max(-step, Math.min(step, wanted - video));
      const offset = audio - video;
      log.push({ t, offset });
      if (t >= nextSample) {
        const noise = (opts.noise ?? 0) * rnd();
        const out = ls.update(
          s(t, offset + noise, { videoTargetMs: Math.max(floorV, opts.videoDelay), audioTargetMs: 60 }),
        );
        if (out) floorV = out.videoMs;
        nextSample = t + ls.nextIntervalMs(t);
      }
    }
    return { ls, log };
  }

  it("비디오가 100ms 앞선 채 시작해도 2.5초부터 ±45ms, 수렴 뒤 ±20ms 안", () => {
    const { log } = simulate({ audioDelay: 130, videoDelay: 30, noise: 15, secs: 30 });
    for (const p of log.filter((p) => p.t >= 2500)) expect(Math.abs(p.offset)).toBeLessThanOrEqual(45);
    for (const p of log.filter((p) => p.t >= 8000)) expect(Math.abs(p.offset)).toBeLessThanOrEqual(20);
  });

  it("바닥은 자연 지연(목표 지연) 위에서부터 더해, 한 번에 따라잡는다", () => {
    const { ls } = simulate({ audioDelay: 130, videoDelay: 30, secs: 5 });
    expect(ls.adjustMs).toBe(130);
  });

  it("처음부터 맞아 있으면 건드리지 않는다", () => {
    // 잡음 ±6 < 데드밴드 8: 첫 샘플 하나로도 움직이지 않는다
    const { ls } = simulate({ audioDelay: 60, videoDelay: 60, noise: 6, secs: 30 });
    expect(ls.adjustMs).toBe(0);
  });
});

describe("목표 보정값", () => {
  it("기본값은 0이다", () => {
    expect(LIPSYNC_TARGET_OFFSET_MS).toBe(0);
    expect(LIPSYNC_DEFAULTS.targetOffsetMs).toBe(0);
  });

  it("차이가 목표 보정값과 같으면 움직이지 않는다", () => {
    const ls = new LipSync({ targetOffsetMs: 30 });
    const s = feed();
    for (let t = 0; t < 10_000; t += 500) expect(ls.update(s(t, 30))).toBeNull();
    expect(ls.adjustMs).toBe(0);
  });

  it("목표 보정값에서 벗어난 만큼만 보정한다", () => {
    const ls = new LipSync({ targetOffsetMs: 30 });
    const s = feed();
    ls.update(s(0, 130));
    expect(ls.update(s(500, 130))).toEqual({ videoMs: 100, audioMs: 0 }); // 130 − 30
  });

  it("목표 보정값보다 낮으면(음수 방향) 오디오 바닥을 올린다", () => {
    const ls = new LipSync({ targetOffsetMs: 30 });
    const s = feed();
    ls.update(s(0, 0));
    expect(ls.update(s(500, 0))).toEqual({ videoMs: 0, audioMs: 30 }); // 0 − 30
  });
});

describe("스트림 변경·초기화", () => {
  it("코덱·해상도가 바뀌면 잠시 재지 않고 보정값은 유지하며, 그 뒤에도 샘플 3개가 모여야 보정한다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // → 100 (첫 보정)
    ls.streamChanged(5000);
    expect(ls.adjustMs).toBe(100);
    expect(ls.update(s(5500, 60))).toBeNull(); // 직전 프레임 수를 지웠으니 기준만 잡는다
    expect(ls.update(s(5999, 60))).toBeNull(); // 대기(6000까지)
    expect(ls.update(s(6000, 60))).toBeNull(); // 첫 샘플
    expect(ls.update(s(6500, 60))).toBeNull(); // 둘째: 아직 부족
    // 차이 60이지만 늘리는 쪽은 한 번에 50까지 → 150 (이전에는 160)
    expect(ls.update(s(7000, 60))).toEqual({ videoMs: 150, audioMs: 0 });
  });

  it("보정한 적이 없으면 코덱이 바뀐 뒤에도 첫 보정은 샘플 1개로 한다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100)); // 기준
    ls.streamChanged(1000);
    expect(ls.update(s(1500, 100))).toBeNull(); // 기준 다시 잡기
    expect(ls.update(s(2000, 100))).toEqual({ videoMs: 100, audioMs: 0 });
  });

  it("reset은 새 연결처럼 처음부터 시작한다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100));
    ls.reset();
    expect(ls.adjustMs).toBe(0);
    expect(ls.floors).toEqual({ videoMs: 0, audioMs: 0 });
    expect(ls.nextIntervalMs(50_000)).toBe(500);
  });

  it("reset 뒤에는 첫 보정이 다시 샘플 1개로 돌아간다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // → 100 (이제 3개가 필요)
    ls.reset();
    expect(ls.update(s(60_000, 100))).toBeNull(); // 기준
    expect(ls.update(s(60_500, 100))).toEqual({ videoMs: 100, audioMs: 0 });
  });
});

describe("반대 방향·방향 전환", () => {
  it("비디오 바닥이 올라가 있을 때 비디오가 늦으면 샘플 하나로 바닥을 내린다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // adjust 100 (2000까지 대기)
    expect(ls.update(s(2000, -40))).toEqual({ videoMs: 60, audioMs: 0 });
  });

  it("바닥이 0인데 비디오가 늦으면 오디오 바닥을 올린다(오디오 목표 지연 위에서)", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, -50, { audioTargetMs: 60 }));
    expect(ls.update(s(500, -50, { audioTargetMs: 60 }))).toEqual({ videoMs: 0, audioMs: 110 });
  });
});

describe("첫 보정과 이후 보정", () => {
  it("첫 보정 뒤에는 샘플 2개로는 움직이지 않고 3개가 모여야 움직인다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    expect(ls.update(s(500, 100))).toEqual({ videoMs: 100, audioMs: 0 }); // 첫 보정: 1개
    // 2000까지 대기, 그 뒤 샘플 3개
    expect(ls.update(s(2000, 100))).toBeNull();
    expect(ls.update(s(2500, 100))).toBeNull();
    // 늘리는 쪽 +50 상한 때문에 200이 아니라 150 (이전 기대값 200)
    expect(ls.update(s(3000, 100))).toEqual({ videoMs: 150, audioMs: 0 });
  });

  it("첫 보정 뒤 샘플 하나만 튀면 중앙값을 써서 크게 움직이지 않는다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // adjust 100
    // 끊김으로 EPT 차이가 한 번 튄다: 평균(101)이 아니라 중앙값(20)을 쓴다
    ls.update(s(2000, 20));
    ls.update(s(2500, 268));
    expect(ls.update(s(3000, 15))).toEqual({ videoMs: 120, audioMs: 0 }); // 100 + 20
    expect(ls.adjustMs).toBe(120);
  });

  it("첫 보정에도 이상값·EPT 없음 조건은 그대로 적용한다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100)); // 기준
    expect(ls.update(s(500, 2500))).toBeNull(); // 이상값
    expect(ls.update(s(1000, null))).toBeNull(); // EPT 없음
    expect(ls.adjustMs).toBe(0);
    expect(ls.update(s(1500, 100))).toEqual({ videoMs: 100, audioMs: 0 });
  });
});

describe("줄이는 방향은 빠르게(reduceMinSamples)", () => {
  it("VP8 끊김 재현: 바닥 300에서 −40, −110, −150, −150이 오면 대기가 끝날 때마다 샘플 하나로 내려간다", () => {
    const ls = new LipSync({ maxStepMs: 400 }); // 300을 한 번에 올리려고만 키운다
    const s = feed();
    ls.update(s(0, 300));
    expect(ls.update(s(500, 300))).toEqual({ videoMs: 300, audioMs: 0 }); // 300 변경 → 3.0 + 0.5초(4000까지) 대기
    expect(ls.update(s(1000, -40))).toBeNull(); // 대기 중
    // 40 변경 → 0.4 + 0.5초(4900까지)
    expect(ls.update(s(4000, -40))).toEqual({ videoMs: 260, audioMs: 0 });
    expect(ls.update(s(4500, -110))).toBeNull(); // 대기 중
    // 110 변경 → 1.1 + 0.5초(6600까지)
    expect(ls.update(s(5000, -110))).toEqual({ videoMs: 150, audioMs: 0 });
    expect(ls.update(s(6000, -150))).toBeNull(); // 대기 중
    // 150 → 0에서 멈춘다(150만큼 줄이려 했지만 바닥은 150뿐). 대기는 |step|=150 기준 1.5 + 0.5초(8600까지)
    expect(ls.update(s(6600, -150))).toEqual({ videoMs: 0, audioMs: 0 });
    expect(ls.adjustMs).toBe(0);
    // 남은 차이는 늘리는 쪽(오디오 바닥) 규칙: 샘플 3개가 모여야 한다
    expect(ls.update(s(8600, -150))).toBeNull();
    expect(ls.update(s(9100, -150))).toBeNull();
    expect(ls.adjustMs).toBe(0);
  });

  it("줄이는 길은 0에서 멈춘다: 바닥 50, 차이 −150 → 0(오디오 바닥을 새로 올리지 않는다)", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 50));
    expect(ls.update(s(500, 50))).toEqual({ videoMs: 50, audioMs: 0 }); // 1.0초(1500까지) 대기
    expect(ls.update(s(1500, -150))).toEqual({ videoMs: 0, audioMs: 0 });
    expect(ls.adjustMs).toBe(0);
  });

  it("오디오 바닥이 올라가 있을 때 비디오가 앞서면 샘플 하나로 내리고 0에서 멈춘다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, -60));
    expect(ls.update(s(500, -60))).toEqual({ videoMs: 0, audioMs: 60 }); // 0.6 + 0.5초(1600까지) 대기
    expect(ls.update(s(1600, 100))).toEqual({ videoMs: 0, audioMs: 0 });
    expect(ls.adjustMs).toBe(0);
  });

  it("줄이는 방향이라도 데드밴드 이하·이상값·정지 구간은 그대로 무시한다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // adjust 100 (2000까지 대기)
    const a = s(2000, -8);
    expect(ls.update(a)).toBeNull(); // 데드밴드(8) 이하
    expect(ls.update(s(2500, -2500))).toBeNull(); // 이상값
    expect(ls.update({ ...s(3000, -60), framesDecoded: a.framesDecoded })).toBeNull(); // 프레임이 늘지 않음
    expect(ls.adjustMs).toBe(100);
  });

  it("늘리는 방향은 여전히 샘플 3개가 필요하다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // adjust 100 (2000까지 대기)
    expect(ls.update(s(2000, 60))).toBeNull();
    expect(ls.update(s(2500, 60))).toBeNull();
    // 차이 60 → +50 상한으로 150 (이전 기대값 160)
    expect(ls.update(s(3000, 60))).toEqual({ videoMs: 150, audioMs: 0 });
  });

  it("reduceMinSamples를 3으로 주면 줄이는 방향도 3개가 필요하다", () => {
    const ls = new LipSync({ reduceMinSamples: 3 });
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // adjust 100 (2000까지 대기)
    expect(ls.update(s(2000, -40))).toBeNull();
    expect(ls.update(s(2500, -40))).toBeNull();
    expect(ls.update(s(3000, -40))).toEqual({ videoMs: 60, audioMs: 0 });
  });
});

describe("데드밴드 8", () => {
  it("차이 9는 보정하고, 7은 무시하며, 8도 무시한다", () => {
    const a = new LipSync();
    const sa = feed();
    a.update(sa(0, 9));
    expect(a.update(sa(500, 9))).toEqual({ videoMs: 9, audioMs: 0 });

    const b = new LipSync();
    const sb = feed();
    for (let t = 0; t < 5000; t += 500) expect(b.update(sb(t, 7))).toBeNull();
    for (let t = 5000; t < 10_000; t += 500) expect(b.update(sb(t, 8))).toBeNull();
    expect(b.adjustMs).toBe(0);
  });

  it("첫 보정 뒤 +9~13ms 잔차도 잡는다(데드밴드 12였다면 남던 값)", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // adjust 100 (2000까지 대기)
    ls.update(s(2000, 10));
    ls.update(s(2500, 10));
    expect(ls.update(s(3000, 10))).toEqual({ videoMs: 110, audioMs: 0 });
  });
});

describe("불안정 구간에는 움직이지 않는다", () => {
  it("기본값(옵션 이름·값의 근거는 lipsync.ts 주석)", () => {
    expect(LIPSYNC_DEFAULTS).toMatchObject({
      concealMinSamples: 960,
      startupMs: 2000,
      triggerHoldMs: 2000,
      spikeMs: 60,
      backlogMs: 60,
      baselineWindow: 10,
      maxUnstableMs: 12_000, // 제안은 8000이었으나 lan/h264의 밀림이 사건 뒤 9.5초 만에 풀려 12000으로 올렸다
      recoverHoldMs: 1500,
      maxFollowStepMs: 50,
      audioTargetJumpMs: 30,
    });
  });

  it("처음 상태는 안정, 이유 없음, 기준 없음", () => {
    const ls = new LipSync();
    expect(ls.debug).toEqual({ state: "stable", reason: null, audioBaseline: null });
  });

  it("은닉 사건이 나면 2초 동안 바닥을 바꾸지 않고, 그 뒤 1.5초 더 안정이어야 다시 모은다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    expect(ls.update(s(500, 100))).toEqual({ videoMs: 100, audioMs: 0 }); // 첫 보정(대기 2000까지)
    expect(ls.update(s(2000, 100))).toBeNull(); // 샘플 [100]
    expect(ls.update(s(2500, 100))).toBeNull(); // 샘플 [100, 100]
    s.concealed = 2000; // +2000 ≥ 960, 시작 2초는 지났다
    expect(ls.update(s(3000, 100))).toBeNull(); // 사건: 모은 샘플을 버린다
    expect(ls.debug).toEqual({ state: "unstable", reason: "conceal", audioBaseline: null });
    // 사건 뒤 2초(3000 ~ 4999)
    for (const t of [3500, 4000, 4500, 4999]) {
      expect(ls.update(s(t, 100))).toBeNull();
      expect(ls.debug.state).toBe("unstable");
    }
    // 5000에 사건 구간이 끝나 회복 중(1.5초 = 6500까지)
    for (const t of [5000, 5500, 6000, 6499]) {
      expect(ls.update(s(t, 100))).toBeNull();
      expect(ls.debug.state).toBe("recovering");
    }
    expect(ls.update(s(6500, 100))).toBeNull(); // 안정으로 돌아옴: 샘플 [100] (첫 보정 뒤라 3개 필요)
    expect(ls.debug.state).toBe("stable");
    expect(ls.update(s(7000, 100))).toBeNull(); // [100, 100]
    expect(ls.update(s(7500, 100))).toEqual({ videoMs: 150, audioMs: 0 }); // 늘리는 쪽 +50 상한
    expect(ls.adjustMs).toBe(150);
  });

  it("은닉 증가가 960 미만이면 사건이 아니고, 960이면 사건이다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 0));
    s.concealed = 959;
    ls.update(s(2000, 0));
    expect(ls.debug.state).toBe("stable");
    s.concealed = 959 + 960;
    ls.update(s(2500, 0));
    expect(ls.debug).toMatchObject({ state: "unstable", reason: "conceal" });
  });

  it("시작 2초 안의 은닉은 무시한다(시작 직후 은닉은 정상)", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    s.concealed = 1920; // 정상 실행에서 시작 직후 480~1920이 는다
    expect(ls.update(s(500, 100))).toEqual({ videoMs: 100, audioMs: 0 }); // 사건이 아니라 첫 보정이 그대로 된다
    expect(ls.debug.state).toBe("stable");

    // 경계: 1999는 시작 구간이라 무시, 2000부터는 사건
    const b = new LipSync();
    const sb = feed();
    b.update(sb(0, 0));
    sb.concealed = 1000;
    b.update(sb(1999, 0));
    expect(b.debug.state).toBe("stable");
    sb.concealed = 2000;
    b.update(sb(2000, 0));
    expect(b.debug).toMatchObject({ state: "unstable", reason: "conceal" });
  });

  it("freezeCount 증가도 사건이다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    expect(ls.update(s(500, 100))).toEqual({ videoMs: 100, audioMs: 0 });
    s.freezes = 1; // 은닉 변화 없이 멈춤만
    expect(ls.update(s(2000, 100))).toBeNull();
    expect(ls.debug).toMatchObject({ state: "unstable", reason: "freeze" });
    expect(ls.update(s(2500, 100))).toBeNull();
    expect(ls.update(s(3500, 100))).toBeNull();
    expect(ls.debug.state).toBe("unstable"); // 2000 + 2000 = 4000까지
  });

  it("멈춤은 시작 2초 안이어도 사건이다(무시하는 것은 은닉뿐)", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    s.freezes = 1;
    for (const t of [500, 1000, 1500, 2000]) expect(ls.update(s(t, 100))).toBeNull(); // 사건 500 + 2000 = 2500까지
    expect(ls.debug).toMatchObject({ state: "unstable", reason: "freeze" });
    expect(ls.adjustMs).toBe(0);
    expect(ls.update(s(2500, 100))).toBeNull(); // 회복 중(4000까지)
    expect(ls.debug.state).toBe("recovering");
    expect(ls.update(s(3999, 100))).toBeNull();
    // 4000: 안정. 한 번도 안 바꿨으니 첫 보정이라 샘플 1개로 바로 한다
    expect(ls.update(s(4000, 100))).toEqual({ videoMs: 100, audioMs: 0 });
    expect(ls.debug.state).toBe("stable");
  });

  it("정상 실행의 오디오 목표 흔들림(40~80ms)은 불안정이 아니다", () => {
    const ls = new LipSync();
    const s = feed();
    // 원자료 server/vp8의 정상 구간: 80 → 43 → 40 → 40 → 72 → 80 ... → 59 → 40 → 48 → 60
    const targets = [80, 43, 40, 40, 72, 80, 80, 80, 59, 40, 48, 60, 60, 43, 40];
    targets.forEach((aT, i) => {
      ls.update(s(i * 500, 0, { audioTargetMs: aT, audioJitterMs: aT + 15 }));
      expect(ls.debug.state).toBe("stable");
    });
  });

  it("오디오 목표 치솟음(기준 40 → 400)이 이어지는 동안 움직이지 않고, 내려온 뒤 1.5초 안정 후에 다시 움직인다", () => {
    const ls = new LipSync();
    const s = feed();
    for (let t = 0; t <= 2500; t += 500) ls.update(s(t, 0, { audioTargetMs: 40 })); // 기준을 모은다(E=0: 데드밴드)
    expect(ls.debug).toEqual({ state: "stable", reason: null, audioBaseline: 40 });
    for (let t = 3000; t <= 7500; t += 500) {
      expect(ls.update(s(t, 100, { audioTargetMs: 400 }))).toBeNull(); // 400 ≥ 40 + 60
      expect(ls.debug).toEqual({ state: "unstable", reason: "audio-spike", audioBaseline: 40 }); // 치솟은 값은 기준에 안 들어간다
    }
    expect(ls.adjustMs).toBe(0);
    // 8000에 목표가 40으로 내려옴 → 회복 중(9500까지)
    for (const t of [8000, 8500, 9000, 9499]) {
      expect(ls.update(s(t, 100, { audioTargetMs: 40 }))).toBeNull();
      expect(ls.debug.state).toBe("recovering");
    }
    // 9500: 안정. 첫 보정이라 샘플 1개로 바로 한다
    expect(ls.update(s(9500, 100, { audioTargetMs: 40 }))).toEqual({ videoMs: 100, audioMs: 0 });
    expect(ls.debug.state).toBe("stable");
  });

  it("밀림(오디오 지연 − 목표 ≥ 60) 동안 움직이지 않는다(경계: 60은 밀림, 59는 아님)", () => {
    const ls = new LipSync();
    const s = feed();
    const ok = { audioTargetMs: 40, audioJitterMs: 60 }; // 지연 − 목표 = 20: 정상
    for (let t = 0; t <= 2500; t += 500) ls.update(s(t, 0, ok));
    expect(ls.debug.state).toBe("stable");
    expect(ls.update(s(3000, 100, { audioTargetMs: 40, audioJitterMs: 100 }))).toBeNull(); // 100 − 40 = 60
    expect(ls.debug).toEqual({ state: "unstable", reason: "audio-backlog", audioBaseline: 40 });
    // 99 − 40 = 59: 풀림 → 회복 중(3500 + 1500 = 5000까지)
    const freed = { audioTargetMs: 40, audioJitterMs: 99 };
    expect(ls.update(s(3500, 100, freed))).toBeNull();
    expect(ls.debug.state).toBe("recovering");
    expect(ls.update(s(4999, 100, freed))).toBeNull();
    expect(ls.update(s(5000, 100, freed))).toEqual({ videoMs: 100, audioMs: 0 });
  });

  it("치솟음이 12초 넘게 이어지면 새 기준으로 받아들이고, 늘리는 쪽은 한 번에 50ms씩 따라간다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100, { audioTargetMs: 40 }));
    expect(ls.update(s(500, 100, { audioTargetMs: 40 }))).toEqual({ videoMs: 100, audioMs: 0 }); // 첫 보정(2000까지 대기)
    ls.update(s(2000, 0, { audioTargetMs: 40 }));
    ls.update(s(2500, 0, { audioTargetMs: 40 }));
    // 3000부터 목표가 140으로 오래 유지된다(변화 후 E = +80). 140 ≥ 40 + 60: 치솟음
    for (let t = 3000; t <= 14_500; t += 500) {
      expect(ls.update(s(t, 80, { audioTargetMs: 140 }))).toBeNull(); // 불안정 3000 + 12000 = 15000 전까지
      expect(ls.debug.state).toBe("unstable");
    }
    expect(ls.adjustMs).toBe(100);
    // 15000: 12초가 지났다 → 140을 새 기준으로 받아들이고 회복 중(16500까지)
    expect(ls.update(s(15_000, 80, { audioTargetMs: 140 }))).toBeNull();
    expect(ls.debug).toEqual({ state: "recovering", reason: "audio-spike", audioBaseline: 140 });
    for (const t of [15_500, 16_000, 16_499]) expect(ls.update(s(t, 80, { audioTargetMs: 140 }))).toBeNull();
    expect(ls.update(s(16_500, 80, { audioTargetMs: 140 }))).toBeNull(); // 안정, 샘플 [80]
    expect(ls.debug.state).toBe("stable");
    expect(ls.update(s(17_000, 80, { audioTargetMs: 140 }))).toBeNull(); // [80, 80]
    // 차이 80이지만 +50까지: 100 → 150 (대기 50/100 + 0.5 = 1.0초 → 18500)
    expect(ls.update(s(17_500, 80, { audioTargetMs: 140 }))).toEqual({ videoMs: 150, audioMs: 0 });
    expect(ls.update(s(18_000, 80, { audioTargetMs: 140 }))).toBeNull(); // 대기 중
    expect(ls.update(s(18_500, 80, { audioTargetMs: 140 }))).toBeNull(); // [80]
    expect(ls.update(s(19_000, 80, { audioTargetMs: 140 }))).toBeNull(); // [80, 80]
    expect(ls.update(s(19_500, 80, { audioTargetMs: 140 }))).toEqual({ videoMs: 200, audioMs: 0 });
  });

  it("첫 보정은 200까지, 그 뒤 늘리는 쪽은 한 번에 50까지(비디오·오디오 모두), 줄이는 쪽은 제한 없음", () => {
    const v = new LipSync();
    const sv = feed();
    v.update(sv(0, 900));
    expect(v.update(sv(500, 900))).toEqual({ videoMs: 200, audioMs: 0 }); // 첫 보정: maxStep 200 (대기 2.0 + 0.5 = 3000까지)
    expect(v.update(sv(3000, 900))).toBeNull();
    expect(v.update(sv(3500, 900))).toBeNull();
    expect(v.update(sv(4000, 900))).toEqual({ videoMs: 250, audioMs: 0 });

    const a = new LipSync();
    const sa = feed();
    a.update(sa(0, -900));
    expect(a.update(sa(500, -900))).toEqual({ videoMs: 0, audioMs: 200 });
    expect(a.update(sa(3000, -900))).toBeNull();
    expect(a.update(sa(3500, -900))).toBeNull();
    expect(a.update(sa(4000, -900))).toEqual({ videoMs: 0, audioMs: 250 });

    // 줄이는 쪽: 200에서 차이 −150이면 한 번에 150을 내려 50이 된다
    const r = new LipSync();
    const sr = feed();
    r.update(sr(0, 900));
    expect(r.update(sr(500, 900))).toEqual({ videoMs: 200, audioMs: 0 });
    expect(r.update(sr(3000, -150))).toEqual({ videoMs: 50, audioMs: 0 });
  });

  it("오디오 목표가 직전 샘플보다 30ms 이상 바뀌면 모은 샘플을 버린다(29는 버리지 않는다)", () => {
    const run = (secondTarget: number) => {
      const ls = new LipSync();
      const s = feed();
      ls.update(s(0, 100, { audioTargetMs: 40 }));
      expect(ls.update(s(500, 100, { audioTargetMs: 40 }))).toEqual({ videoMs: 100, audioMs: 0 }); // 2000까지 대기
      ls.update(s(2000, 60, { audioTargetMs: 40 })); // [60]
      ls.update(s(2500, 60, { audioTargetMs: 40 })); // [60, 60]
      return { ls, s, out: ls.update(s(3000, 60, { audioTargetMs: secondTarget })) };
    };
    // 40 → 69: 29 → 버리지 않아 [60, 60, 60]이 되어 +50
    expect(run(69).out).toEqual({ videoMs: 150, audioMs: 0 });
    // 40 → 70: 30 → 버리고 현재 샘플만 [60]. 대기는 걸지 않아 두 개 더 모이면 움직인다
    const { ls, s, out } = run(70);
    expect(out).toBeNull();
    expect(ls.debug.state).toBe("stable"); // 70은 치솟음(기준 40 + 60 = 100 이상)이 아니다
    expect(ls.update(s(3500, 60, { audioTargetMs: 70 }))).toBeNull(); // [60, 60]
    expect(ls.update(s(4000, 60, { audioTargetMs: 70 }))).toEqual({ videoMs: 150, audioMs: 0 });
  });

  it("reset하면 불안정 상태와 기준도 지운다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 0, { audioTargetMs: 40 }));
    s.freezes = 1;
    ls.update(s(500, 0, { audioTargetMs: 40 }));
    expect(ls.debug.state).toBe("unstable");
    ls.reset();
    expect(ls.debug).toEqual({ state: "stable", reason: null, audioBaseline: null });
  });
});

/** 원자료 순서열을 새 보정기에 차례로 넣는다(열린 고리: 보정기가 바닥을 바꿔도 다음 값은 달라지지 않는다). */
function replay(rows: LipSyncFixtureRow[]) {
  const ls = new LipSync();
  const changes: { t: number; floors: LipSyncFloors }[] = [];
  const trace = new Map<number, { state: string; reason: string | null }>();
  for (const r of rows) {
    const out = ls.update({
      now: r.t,
      avOffsetMs: r.avOffsetMs,
      framesDecoded: r.framesDecoded,
      audioPackets: r.audioPackets,
      videoTargetMs: r.videoTargetMs,
      audioTargetMs: r.audioTargetMs,
      audioJitterMs: r.audioJitterMs,
      audioConcealedSamples: r.concealedSamples,
      freezeCount: r.freezeCount,
    });
    if (out) changes.push({ t: r.t, floors: out });
    const { state, reason } = ls.debug;
    trace.set(r.t, { state, reason });
  }
  return { ls, changes, trace };
}

describe("원자료 재생(열린 고리: 옛 보정기가 걸고 있던 때의 값이라 E는 그 영향을 받았다)", () => {
  it("lan/h264: 끊김(4.0초) 뒤 오디오 지연이 빠질 때까지 바닥이 한 번도 안 바뀌고, 바닥은 첫 보정 + 50을 넘지 않는다", () => {
    const { changes, trace } = replay(LIPSYNC_FIXTURE_LAN_H264);
    // 첫 보정: 2009에 E=+13 하나로. 바닥 = max(0, 비디오 목표 19) + 13 = 32
    expect(changes[0]).toEqual({ t: 2009, floors: { videoMs: 32, audioMs: 0 } });
    // 4.0초(사건) ~ 13.5초(밀림이 풀림) + 회복 1.5초 = 15.0초까지 바뀐 것이 없다. (15008에 안정이 되어 그때부터 다시 본다.)
    expect(changes.filter((c) => c.t >= 4000 && c.t <= 15_000)).toEqual([]);
    expect(Math.max(...changes.map((c) => c.floors.videoMs))).toBeLessThanOrEqual(changes[0].floors.videoMs + 50);
    // 상태: 사건 → 치솟음 → 밀림 → 회복 → 안정
    expect(trace.get(2009)).toEqual({ state: "stable", reason: null });
    expect(trace.get(4009)).toEqual({ state: "unstable", reason: "conceal" });
    // 7009에 freezeCount가 1 → 2(두 번째 멈춤)라 9009까지는 사건 보류가 이어진다.
    expect(trace.get(8010)).toEqual({ state: "unstable", reason: "freeze" });
    expect(trace.get(9508)).toEqual({ state: "unstable", reason: "audio-spike" }); // 목표 400 ≥ 40 + 60
    expect(trace.get(13_008)).toEqual({ state: "unstable", reason: "audio-backlog" }); // 지연 121 − 목표 40 = 81
    expect(trace.get(13_508)).toEqual({ state: "recovering", reason: "audio-backlog" }); // 97 − 40 = 57 < 60
    expect(trace.get(14_508)?.state).toBe("recovering");
    expect(trace.get(15_008)).toEqual({ state: "stable", reason: null });
  });

  it("server/vp8: 시작 은닉은 무시하고, 20.0초 끊김 뒤로 바닥이 늘지 않는다", () => {
    const { changes, trace } = replay(LIPSYNC_FIXTURE_SERVER_VP8);
    expect(trace.get(507)).toEqual({ state: "stable", reason: null }); // 은닉 +1440이지만 시작 2초 안
    expect(changes[0]).toEqual({ t: 2006, floors: { videoMs: 106, audioMs: 0 } }); // max(0, 13) + 93
    expect(Math.max(...changes.map((c) => c.floors.videoMs))).toBeLessThanOrEqual(changes[0].floors.videoMs + 50);
    expect(trace.get(20_011)).toEqual({ state: "unstable", reason: "conceal" });
    expect(trace.get(25_005)?.state).toBe("unstable"); // 목표 100 ≥ 40 + 60이 끝까지 이어진다
    expect(changes.filter((c) => c.t >= 20_000)).toEqual([]);
  });
});

describe("닫힌 고리 모형(끊김·지연 변화)", () => {
  /**
   * 모의 브라우저: 비디오 지연은 바닥/자연 지연(30ms)을 초당 100ms로 따라간다. E = 오디오 실제 지연 − 비디오 지연.
   * 오디오는 `audioActual`(실제 재생 지연)과 `audioTarget`(NetEq 목표)로 흉내 낸다. 사건이 나면 은닉 +7440·멈춤 +1.
   * 잡음 없음(기대값을 손으로 따지기 위해).
   */
  function simulateFault(opts: {
    audioActual: (t: number) => number;
    audioTarget: (t: number) => number;
    eventAt?: number;
    secs: number;
  }) {
    const ls = new LipSync();
    const s = feed();
    const videoNatural = 30;
    let video = videoNatural;
    let floorV = 0;
    const log: { t: number; offset: number }[] = [];
    const moves: { t: number; videoMs: number }[] = [];
    const dt = 100;
    let nextSample = 0;
    for (let t = 0; t <= opts.secs * 1000; t += dt) {
      const wanted = Math.max(floorV, videoNatural);
      const step = (100 * dt) / 1000;
      video += Math.max(-step, Math.min(step, wanted - video));
      const offset = opts.audioActual(t) - video;
      log.push({ t, offset });
      if (t >= nextSample) {
        if (opts.eventAt !== undefined && t >= opts.eventAt) {
          s.concealed = 7440;
          s.freezes = 1;
        }
        const out = ls.update(
          s(t, offset, {
            videoTargetMs: Math.max(floorV, videoNatural),
            audioTargetMs: opts.audioTarget(t),
            audioJitterMs: opts.audioActual(t),
          }),
        );
        if (out) {
          floorV = out.videoMs;
          moves.push({ t, videoMs: out.videoMs });
        }
        nextSample = t + ls.nextIntervalMs(t);
      }
    }
    return { ls, log, moves };
  }

  it("6초에 오디오 지연이 +300 뛰고(사건 동반) 5초 유지 후 3초에 걸쳐 빠져도, 보정이 일을 키우지 않는다", () => {
    // 오디오 실제 지연: 130 → (6초) 430 → (11초부터 3초에 걸쳐) 130. 목표는 11초에 바로 130으로 내려간다(원자료와 같은 모양).
    const { ls, log, moves } = simulateFault({
      audioActual: (t) => (t < 6000 ? 130 : t < 11_000 ? 430 : t < 14_000 ? 430 - (t - 11_000) / 10 : 130),
      audioTarget: (t) => (t >= 6000 && t < 11_000 ? 430 : 130),
      eventAt: 6000,
      secs: 40,
    });
    // 500에 첫 보정(E=+100 → 바닥 max(0, 30) + 100 = 130)뿐, 사건·치솟음·밀림·회복 동안 한 번도 안 움직인다.
    expect(moves).toEqual([{ t: 500, videoMs: 130 }]);
    // 지나침 없음: 2.5초부터 E가 −45 아래로 내려가지 않는다(움직이지 않았으니 빠진 뒤 E는 0으로 돌아온다)
    for (const p of log.filter((p) => p.t >= 2500)) expect(p.offset).toBeGreaterThanOrEqual(-45);
    // 지연이 다 빠진 14초부터는 ±20 안(회복 뒤 12초 안, 실제로는 바로)
    for (const p of log.filter((p) => p.t >= 14_000)) expect(Math.abs(p.offset)).toBeLessThanOrEqual(20);
    expect(ls.adjustMs).toBe(130);
    expect(ls.debug.state).toBe("stable");
  });

  it("사건 없이 오래 이어지는 +80ms 지연 변화는 결국(30초 안) ±20ms로 따라간다", () => {
    const { log, moves } = simulateFault({
      audioActual: (t) => (t < 6000 ? 130 : 210),
      audioTarget: (t) => (t < 6000 ? 130 : 210),
      secs: 40,
    });
    // 첫 보정 130(500) → 6초부터 치솟음(210 ≥ 130 + 60)으로 보류 → 18000에 새 기준으로 받아들임 → 20000부터 안정
    // → 22000(샘플 3개 모임, E=+80 → +50 상한) 180 → 25000(E=+30) 210
    expect(moves).toEqual([
      { t: 500, videoMs: 130 },
      { t: 22_000, videoMs: 180 },
      { t: 25_000, videoMs: 210 },
    ]);
    for (const p of log.filter((p) => p.t >= 2500 && p.t < 6000)) expect(Math.abs(p.offset)).toBeLessThanOrEqual(45);
    for (const p of log.filter((p) => p.t >= 26_000)) expect(Math.abs(p.offset)).toBeLessThanOrEqual(20);
  });
});
