import { describe, expect, it } from "vitest";
import { LIPSYNC_DEFAULTS, LIPSYNC_TARGET_OFFSET_MS, LipSync, type LipSyncSample } from "./lipsync";

/** 샘플을 만든다. 프레임·오디오 패킷 수는 호출마다 늘어난다(멈춘 구간을 시험할 때만 고정). */
function feed() {
  let frames = 0;
  let packets = 0;
  return (now: number, avOffsetMs: number | null, over: Partial<LipSyncSample> = {}): LipSyncSample => {
    frames += 15;
    packets += 25;
    return {
      now,
      avOffsetMs,
      framesDecoded: frames,
      audioPackets: packets,
      videoTargetMs: null,
      audioTargetMs: null,
      ...over,
    };
  };
}

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
    expect(ls.update(s(2000, 100))).toBeNull();
    expect(ls.update(s(2500, 100))).toBeNull();
    expect(ls.update(s(3000, 100))).toEqual({ videoMs: 200, audioMs: 0 });
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
    const { ls } = simulate({ audioDelay: 60, videoDelay: 60, noise: 10, secs: 30 });
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
    expect(ls.update(s(7000, 60))).toEqual({ videoMs: 160, audioMs: 0 });
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
  it("비디오 바닥이 올라가 있을 때 비디오가 늦으면 바닥을 내린다", () => {
    const ls = new LipSync();
    const s = feed();
    ls.update(s(0, 100));
    ls.update(s(500, 100)); // adjust 100 (2000까지 대기)
    ls.update(s(2000, -40));
    ls.update(s(2500, -40));
    expect(ls.update(s(3000, -40))).toEqual({ videoMs: 60, audioMs: 0 });
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
    expect(ls.update(s(3000, 100))).toEqual({ videoMs: 200, audioMs: 0 });
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
