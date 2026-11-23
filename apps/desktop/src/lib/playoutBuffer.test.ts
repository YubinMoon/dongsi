import { describe, expect, it } from "vitest";
import {
  PLAYOUT_BUFFER_DEFAULTS,
  PlayoutBuffer,
  audioStall,
  videoArrivalLateMs,
  videoLateMs,
  videoStallMs,
} from "./playoutBuffer";

/**
 * 올림·내림 규칙을 보는 시험용 값(D-072 때 값: 바닥 100, 상한 300, 20초 조용하면 내림, 기다림 최대 5분). 규칙은 값과 상관없이 같고,
 * 지금 기본값(D-094)은 "기본값" 묶음에서 본다.
 */
const RULES = { baseMs: 100, maxMs: 300, quietMs: 20_000, maxQuietMs: 300_000 };

/** 시작 기준(firstAt = 0)을 잡은 규칙 시험용 버퍼 */
function started(): PlayoutBuffer {
  const b = new PlayoutBuffer(RULES);
  b.tick(0);
  return b;
}

describe("PlayoutBuffer 기본값", () => {
  it("D-094: 시작·바닥 150ms, 최대 500ms, 한 번에 최대 100ms, 10초마다 15ms씩 내림, 기다림 10초~1분", () => {
    expect(PLAYOUT_BUFFER_DEFAULTS.baseMs).toBe(150);
    expect(PLAYOUT_BUFFER_DEFAULTS.maxMs).toBe(500);
    expect(PLAYOUT_BUFFER_DEFAULTS.maxRaiseStepMs).toBe(100);
    expect(PLAYOUT_BUFFER_DEFAULTS.lowerStepMs).toBe(15);
    expect(PLAYOUT_BUFFER_DEFAULTS.quietMs).toBe(10_000);
    expect(PLAYOUT_BUFFER_DEFAULTS.maxQuietMs).toBe(60_000);
  });

  it("처음엔 150ms이고, 기본값에서는 내리지 않는다", () => {
    const b = new PlayoutBuffer();
    b.tick(0);
    expect(b.ms).toBe(150);
    expect(b.tick(1_000_000)).toBeNull();
    expect(b.ms).toBe(150);
  });

  it("기본값에서 끊기면 150ms부터 늦은 만큼 올리고, 10초마다 15ms씩 150ms까지 내린다", () => {
    const b = new PlayoutBuffer();
    b.tick(0);
    expect(b.stall(5000, 40, "audio")).toBeNull(); // 50ms 미만은 그대로
    expect(b.stall(5000, 60, "audio")).toBe(210);
    const seen: number[] = [];
    for (let t = 15_000; t <= 65_000; t += 10_000) {
      const v = b.tick(t);
      if (v !== null) seen.push(v);
    }
    expect(seen).toEqual([195, 180, 165, 150]);
    expect(b.ms).toBe(150);
  });

  it("기본값: 500ms에서 끊김이 없으면 10초마다 내려 약 4분 뒤 150ms", () => {
    const b = new PlayoutBuffer();
    b.tick(0);
    for (const t of [3000, 6000, 9000, 12_000]) b.stall(t, 100, "video");
    expect(b.ms).toBe(500);
    let t = 12_000;
    let steps = 0;
    while (b.ms > 150 && steps < 100) {
      t += 10_000;
      if (b.tick(t) !== null) steps++;
    }
    expect(b.ms).toBe(150);
    expect(steps).toBe(24); // 350 / 15 = 23.3 → 24번, 240초
  });

  it("기본값: 되돌이 기다림은 1분까지만 늘어난다", () => {
    const b = new PlayoutBuffer();
    b.tick(0);
    let t = 5000;
    b.stall(t, 100, "audio");
    for (let i = 0; i < 4; i++) {
      t += b.quietMs;
      expect(b.tick(t)).not.toBeNull();
      t += 1000;
      b.stall(t, 100, "audio");
    }
    expect(b.quietMs).toBe(60_000);
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
    const b = new PlayoutBuffer(RULES);
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

describe("audioStall", () => {
  const d = (concealedMs: number, lost = 0, received = 100) => ({
    concealedSamples: concealedMs * 48,
    packetsReceived: received,
    packetsLost: lost,
  });

  it("(a) 손실 7패킷 + 은닉 70ms: 전부 손실이라 늦음 없음", () => {
    const s = audioStall(d(70, 7))!;
    expect(s.concealedMs).toBeCloseTo(70, 5);
    expect(s.lossMs).toBeCloseTo(70, 5);
    expect(s.lateMs).toBeNull();
  });

  it("(b) 손실 0 + 은닉 120ms: 늦음 120", () => {
    expect(audioStall(d(120))!.lateMs).toBeCloseTo(120, 5);
  });

  it("(c) 손실 5 + 은닉 200ms: 늦음 150", () => {
    const s = audioStall(d(200, 5))!;
    expect(s.lossMs).toBeCloseTo(50, 5);
    expect(s.lateMs).toBeCloseTo(150, 5);
  });

  it("(d) 손실 0·은닉 30ms: 늦음 30(버린 패킷은 이미 은닉에 들어 있어 따로 세지 않는다)", () => {
    expect(audioStall(d(30))!.lateMs).toBeCloseTo(30, 5);
  });

  it("(d1) RED: 중복 블록 버림이 늘고(입력에 없음) 손실 20·은닉 0이면 null", () => {
    expect(audioStall(d(0, 20))).toBeNull();
  });

  it("(d2) RED: 연속 3손실에 은닉 10ms면 손실 몫 30ms에 가려 null", () => {
    const s = audioStall(d(10, 3));
    expect(s === null || s.lateMs === null).toBe(true);
  });

  it("(e) packetsLost가 음수면 0으로", () => {
    const s = audioStall(d(120, -3))!;
    expect(s.lossMs).toBe(0);
    expect(s.lateMs).toBeCloseTo(120, 5);
  });

  it("(f) 받은 패킷이 없거나 은닉이 없으면 null", () => {
    expect(audioStall(d(120, 0, 0))).toBeNull();
    expect(audioStall(d(0, 5))).toBeNull();
  });

  it("샘플레이트·패킷 길이를 바꿀 수 있다(20ms 패킷)", () => {
    const s = audioStall(d(100, 2), { samplesPerPacket: 960 })!;
    expect(s.lossMs).toBeCloseTo(40, 5);
    expect(s.lateMs).toBeCloseTo(60, 5);
  });

  it("(g) 손실만(RED로 손실이 많고 은닉 0) 이어지는 구간이 여러 번 와도 B는 시작값 그대로", () => {
    for (const opts of [{}, RULES]) {
      const b = new PlayoutBuffer(opts);
      b.tick(0);
      const start = b.ms;
      for (let i = 1; i <= 12; i++) {
        const now = 1000 * i + 2000;
        // 손실 몫으로 다 설명되는 은닉, RED가 복구해 은닉이 0인 손실
        for (const delta of [d(100, 10), d(0, 20)]) {
          const s = audioStall(delta);
          if (s?.lateMs != null) b.stall(now, s.lateMs, "audio");
        }
        b.tick(now);
      }
      expect(b.ms).toBe(start);
      expect(b.events).toEqual([]);
    }
  });
});

describe("비디오 끊김 두 경우", () => {
  const f = (displayMs: number, rtp: number, receiveMs: number) => ({ displayMs, rtp, receiveMs });

  it("키프레임 대기: 새로 캡처된 키프레임이라 RTP 간격도 같이 길어 세지 않는다", () => {
    // 마지막 프레임 뒤 1.2초 멈췄다가 키프레임이 나왔다(캡처도 1.2초 뒤)
    const prev = f(1000, 90_000, 950);
    const cur = f(2200, 90_000 + 108_000, 2150);
    expect(videoLateMs(prev, cur)).toBeCloseTo(0, 5);
    expect(videoArrivalLateMs(prev, cur)).toBeCloseTo(0, 5);
    const late = videoStallMs(prev, cur);
    expect(late === null || late < PLAYOUT_BUFFER_DEFAULTS.minStallMs).toBe(true);
    const b = new PlayoutBuffer();
    b.tick(0);
    if (late !== null) b.stall(5000, late, "video");
    expect(b.ms).toBe(150);
  });

  it("재전송(RTX) 대기: RTT만큼 늦게 받아 늦음으로 센다", () => {
    // 33ms 간격 프레임이 RTT 120ms를 더 기다려 다 모였다
    const prev = f(1000, 90_000, 950);
    const cur = f(1153, 93_000, 1103);
    expect(videoArrivalLateMs(prev, cur)).toBeCloseTo(120, 0);
    const late = videoStallMs(prev, cur);
    expect(late).toBeCloseTo(120, 0);
    const b = new PlayoutBuffer();
    b.tick(0);
    expect(b.stall(5000, late!, "video")).toBe(250); // 150 + 한 번에 최대 100
  });
});

describe("시작값과 바닥", () => {
  it("startMs는 [baseMs, maxMs]로 자른다", () => {
    expect(new PlayoutBuffer({ baseMs: 40, startMs: 10 }).ms).toBe(40);
    expect(new PlayoutBuffer({ baseMs: 40, startMs: 900 }).ms).toBe(500);
  });

  it("개발용 끄개 { baseMs: 0, maxMs: 0 }는 0 고정", () => {
    const b = new PlayoutBuffer({ baseMs: 0, maxMs: 0 });
    b.tick(0);
    expect(b.ms).toBe(0);
    expect(b.stall(5000, 100, "audio")).toBeNull();
    expect(b.ms).toBe(0);
    b.reset();
    expect(b.ms).toBe(0);
  });
});
