import { describe, expect, it } from "vitest";
import {
  PLAYOUT_BUFFER_DEFAULTS,
  PlayoutBuffer,
  audioLateMs,
  videoArrivalLateMs,
  videoLateMs,
  videoStallMs,
} from "./playoutBuffer";

/** 시작 기준(firstAt = 0)을 잡은 버퍼 */
function started(): PlayoutBuffer {
  const b = new PlayoutBuffer();
  b.tick(0);
  return b;
}

describe("PlayoutBuffer 기본값", () => {
  it("D-072: 기본 100ms, 최대 300ms, 한 번에 최대 100ms, 15ms씩 내림, 기다림 20초~5분", () => {
    expect(PLAYOUT_BUFFER_DEFAULTS.baseMs).toBe(100);
    expect(PLAYOUT_BUFFER_DEFAULTS.maxMs).toBe(300);
    expect(PLAYOUT_BUFFER_DEFAULTS.maxRaiseStepMs).toBe(100);
    expect(PLAYOUT_BUFFER_DEFAULTS.lowerStepMs).toBe(15);
    expect(PLAYOUT_BUFFER_DEFAULTS.quietMs).toBe(20_000);
    expect(PLAYOUT_BUFFER_DEFAULTS.maxQuietMs).toBe(300_000);
  });

  it("처음엔 100ms이고, 기본값에서는 내리지 않는다", () => {
    const b = started();
    expect(b.ms).toBe(100);
    expect(b.tick(1_000_000)).toBeNull();
    expect(b.ms).toBe(100);
  });
});

describe("PlayoutBuffer 올리기", () => {
  it("늦은 만큼 올린다", () => {
    const b = started();
    expect(b.stall(5000, 80, "video")).toBe(180);
    expect(b.ms).toBe(180);
  });

  it("한 번에 100ms까지, 전체 300ms까지", () => {
    const b = started();
    expect(b.stall(5000, 250, "audio")).toBe(200);
    expect(b.stall(8000, 250, "audio")).toBe(300);
    expect(b.stall(11000, 250, "audio")).toBeNull();
    expect(b.ms).toBe(300);
  });

  it("시작 직후(2초)의 끊김은 보지 않는다", () => {
    const b = new PlayoutBuffer();
    expect(b.stall(0, 100, "audio")).toBeNull(); // 처음 부른 시각이 기준
    expect(b.stall(1999, 100, "audio")).toBeNull();
    expect(b.stall(2000, 100, "audio")).toBe(200);
  });

  it("50ms 미만, 2초 초과, 숫자가 아닌 값은 무시한다", () => {
    const b = started();
    expect(b.stall(5000, 49, "video")).toBeNull();
    expect(b.stall(5000, 2001, "video")).toBeNull();
    expect(b.stall(5000, Number.NaN, "video")).toBeNull();
    expect(b.ms).toBe(100);
    expect(b.stall(5000, 50, "video")).toBe(150);
  });

  it("hold 동안은 끊김을 보지 않는다", () => {
    const b = started();
    b.hold(5000, 1000);
    expect(b.stall(5999, 80, "video")).toBeNull();
    expect(b.stall(6000, 80, "video")).toBe(180);
  });

  it("올린 뒤 2초 안의 끊김은 하나로 묶는다(묶음을 시작할 때의 값에서 잰다)", () => {
    const b = started();
    expect(b.stall(5000, 60, "video")).toBe(160);
    // 버퍼를 올리면서 생긴 멈춤(같은 크기)은 더 올리지 않는다
    expect(b.stall(5500, 60, "video")).toBeNull();
    // 같은 묶음 안에서 더 크게 늦었으면 그만큼만 더
    expect(b.stall(6000, 90, "audio")).toBe(190);
    // 묶음 안에서도 한 번에 올리는 양(100ms)은 묶음 시작 값 기준
    expect(b.stall(6500, 300, "audio")).toBe(200);
    // 묶음이 끝난 뒤는 지금 값에서 다시
    expect(b.stall(7100, 50, "video")).toBe(250);
  });
});

describe("PlayoutBuffer 내리기", () => {
  it("20초 동안 조용하면 15ms씩, 100ms까지 내린다", () => {
    const b = started();
    b.stall(5000, 80, "video"); // 180
    expect(b.tick(24_999)).toBeNull();
    expect(b.tick(25_000)).toBe(165);
    expect(b.tick(44_999)).toBeNull();
    expect(b.tick(45_000)).toBe(150);
    const seen: number[] = [];
    for (let t = 65_000; t <= 165_000; t += 20_000) {
      const v = b.tick(t);
      if (v !== null) seen.push(v);
    }
    expect(seen).toEqual([135, 120, 105, 100]);
    expect(b.ms).toBe(100);
  });

  it("최댓값에서 끊겨 올리지 못해도 조용한 시간은 다시 센다", () => {
    const b = started();
    b.stall(5000, 100, "audio"); // 200
    b.stall(8000, 100, "audio"); // 300
    expect(b.stall(11_000, 100, "audio")).toBeNull();
    expect(b.tick(30_999)).toBeNull();
    expect(b.tick(31_000)).toBe(285);
  });

  it("내린 뒤 60초 안에 다시 끊기면 기다림을 두 배로", () => {
    const b = started();
    b.stall(5000, 100, "video"); // 200
    expect(b.tick(25_000)).toBe(185);
    expect(b.stall(30_000, 60, "video")).toBe(245);
    expect(b.quietMs).toBe(40_000);
    expect(b.tick(69_999)).toBeNull();
    expect(b.tick(70_000)).toBe(230);
  });

  it("되돌이는 내림 한 번에 한 번만 센다", () => {
    const b = started();
    b.stall(5000, 100, "video");
    b.tick(25_000);
    b.stall(30_000, 60, "video");
    b.stall(33_000, 60, "video");
    expect(b.quietMs).toBe(40_000);
  });

  it("내린 뒤 60초가 지나 끊기면 되돌이가 아니다", () => {
    const b = started();
    b.stall(5000, 100, "video");
    b.tick(25_000);
    b.stall(85_000, 60, "video");
    expect(b.quietMs).toBe(20_000);
  });

  it("기다림은 5분까지만 늘어난다", () => {
    const b = started();
    let t = 5000;
    b.stall(t, 100, "audio");
    for (let i = 0; i < 6; i++) {
      t += b.quietMs;
      expect(b.tick(t)).not.toBeNull();
      t += 1000;
      b.stall(t, 100, "audio");
    }
    expect(b.quietMs).toBe(300_000);
  });

  it("10분 동안 끊김이 없으면 기다림을 처음 값으로", () => {
    const b = started();
    b.stall(5000, 100, "video");
    b.tick(25_000);
    b.stall(30_000, 60, "video");
    expect(b.quietMs).toBe(40_000);
    b.tick(629_999);
    expect(b.quietMs).toBe(40_000);
    b.tick(630_000);
    expect(b.quietMs).toBe(20_000);
  });

  it("reset은 처음 상태로", () => {
    const b = started();
    b.stall(5000, 100, "video");
    b.tick(25_000);
    b.stall(30_000, 60, "video");
    b.reset();
    expect(b.ms).toBe(100);
    expect(b.quietMs).toBe(20_000);
    expect(b.events).toEqual([]);
    expect(b.stall(1000, 100, "audio")).toBeNull(); // 시작 직후로 돌아갔다
  });

  it("기록: 끊김과 내림", () => {
    const b = started();
    b.stall(5000, 80.4, "video");
    b.tick(25_000);
    expect(b.events).toEqual([
      { t: 5000, type: "stall", kind: "video", lateMs: 80, ms: 180, quietMs: 20_000 },
      { t: 25_000, type: "lower", kind: null, lateMs: null, ms: 165, quietMs: 20_000 },
    ]);
  });
});

describe("videoLateMs", () => {
  const f = (displayMs: number, rtp: number) => ({ displayMs, rtp });

  it("제때 나온 프레임은 0 근처", () => {
    expect(videoLateMs(f(1000, 90_000), f(1016.7, 91_500))).toBeCloseTo(0, 0);
  });

  it("정지 화면(송신 간격이 김)은 늦은 것이 아니다", () => {
    expect(videoLateMs(f(1000, 90_000), f(2005, 180_000))).toBeCloseTo(5, 5);
  });

  it("늦게 나온 프레임은 표시 간격 − 송신 간격", () => {
    expect(videoLateMs(f(1000, 90_000), f(1150, 91_500))).toBeCloseTo(133.3, 1);
  });

  it("RTP 32비트 넘김", () => {
    expect(videoLateMs(f(1000, 0xffff_fa00), f(1100, 0x0000_0000 + 0x2e))).toBeCloseTo(100 - (0x600 + 0x2e) / 90, 5);
  });

  it("RTP가 뒤로 가거나 표시가 거꾸로면 null", () => {
    expect(videoLateMs(f(1000, 91_500), f(1016, 90_000))).toBeNull();
    expect(videoLateMs(f(1000, 90_000), f(990, 91_500))).toBeNull();
  });
});

describe("videoStallMs", () => {
  const f = (displayMs: number, rtp: number, receiveMs: number | null) => ({ displayMs, rtp, receiveMs });

  it("늦게 받아 늦게 나온 프레임: 화면에 늦은 정도", () => {
    // 33ms 간격 프레임을 150ms 늦게 받았고, 버퍼 여유 50ms를 빼고 100ms 늦게 나왔다
    const prev = f(1000, 90_000, 950);
    const cur = f(1133, 93_000, 1133);
    expect(videoArrivalLateMs(prev, cur)).toBeCloseTo(150, 0);
    expect(videoStallMs(prev, cur)).toBeCloseTo(100, 0);
  });

  it("제때 받았는데 늦게 나온 프레임(디코딩·그리기 멈춤)은 null", () => {
    const prev = f(1000, 90_000, 950);
    const cur = f(1133, 93_000, 983);
    expect(videoLateMs(prev, cur)).toBeCloseTo(100, 0);
    expect(videoStallMs(prev, cur)).toBeNull();
  });

  it("받은 시각이 없으면 화면 기준만 본다", () => {
    expect(videoStallMs(f(1000, 90_000, null), f(1133, 93_000, null))).toBeCloseTo(100, 0);
    expect(videoArrivalLateMs(f(1000, 90_000, null), f(1133, 93_000, 1100))).toBeNull();
  });
});

describe("audioLateMs", () => {
  it("은닉 샘플 / 48 = ms", () => {
    expect(audioLateMs(4800, 25)).toBe(100);
  });
  it("패킷이 안 왔거나 은닉이 없으면 null", () => {
    expect(audioLateMs(4800, 0)).toBeNull();
    expect(audioLateMs(0, 25)).toBeNull();
  });
});
