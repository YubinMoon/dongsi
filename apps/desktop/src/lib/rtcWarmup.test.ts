import { beforeEach, describe, expect, it } from "vitest";
import { resetWarmUpForTests, warmUpWebRtc } from "./rtcWarmup";

/** 만들어진 횟수와 닫힌 횟수를 세는 가짜 생성자 */
function fakeCtor() {
  const calls = { created: 0, closed: 0 };
  class Fake {
    constructor() {
      calls.created++;
    }
    close() {
      calls.closed++;
    }
  }
  return { Ctor: Fake, calls };
}

describe("warmUpWebRtc", () => {
  beforeEach(() => resetWarmUpForTests());

  it("PC 하나를 만들고 바로 닫는다", () => {
    const { Ctor, calls } = fakeCtor();
    expect(warmUpWebRtc(Ctor)).toBe(true);
    expect(calls).toEqual({ created: 1, closed: 1 });
  });

  it("몇 번을 불러도 한 번만 만든다", () => {
    const { Ctor, calls } = fakeCtor();
    warmUpWebRtc(Ctor);
    expect(warmUpWebRtc(Ctor)).toBe(false);
    warmUpWebRtc(Ctor);
    expect(calls).toEqual({ created: 1, closed: 1 });
  });

  it("생성자가 없어도 던지지 않고, 나중에 생성자가 생기면 그때 한 번 한다", () => {
    expect(warmUpWebRtc(undefined)).toBe(false);
    const { Ctor, calls } = fakeCtor();
    expect(warmUpWebRtc(Ctor)).toBe(true);
    expect(calls.created).toBe(1);
  });

  it("생성자가 던져도 삼키고, 다시 시도하지 않는다", () => {
    let tries = 0;
    class Throws {
      constructor() {
        tries++;
        throw new Error("boom");
      }
      close() {}
    }
    expect(warmUpWebRtc(Throws)).toBe(false);
    expect(warmUpWebRtc(Throws)).toBe(false);
    expect(tries).toBe(1);
  });

  it("close가 던져도 삼킨다", () => {
    class BadClose {
      close() {
        throw new Error("boom");
      }
    }
    expect(warmUpWebRtc(BadClose)).toBe(true);
  });

  it("인자를 주지 않으면 전역 RTCPeerConnection을 쓴다(없는 환경에서도 던지지 않는다)", () => {
    expect(() => warmUpWebRtc()).not.toThrow();
  });
});
