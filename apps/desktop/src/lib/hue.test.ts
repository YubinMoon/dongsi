import { describe, expect, it } from "vitest";
import { hueFor, tileVars } from "./hue";

describe("hueFor", () => {
  it("같은 문자열은 늘 같은 값을 돌려준다", () => {
    expect(hueFor("abc")).toBe(hueFor("abc"));
    expect(hueFor("준호")).toBe(hueFor("준호"));
  });

  it("FNV-1a 32비트 기준값과 맞는다", () => {
    // 빈 문자열은 오프셋 기준값 0x811c9dc5 = 2166136261 → % 360
    expect(hueFor("")).toBe(2166136261 % 360);
    // "a"의 FNV-1a 32 = 0xe40c292c = 3826002220
    expect(hueFor("a")).toBe(3826002220 % 360);
  });

  it("0~359 정수만 돌려준다", () => {
    const seeds = ["", "a", "b", "준호", "유나", "💡", "x".repeat(500), "broadcaster-1", "broadcaster-2"];
    for (let i = 0; i < 200; i++) seeds.push(`seed-${i}`);
    for (const s of seeds) {
      const h = hueFor(s);
      expect(Number.isInteger(h)).toBe(true);
      expect(h).toBeGreaterThanOrEqual(0);
      expect(h).toBeLessThan(360);
    }
  });

  it("다른 문자열은 대체로 다른 색조로 퍼진다", () => {
    const set = new Set<number>();
    for (let i = 0; i < 100; i++) set.add(hueFor(`seed-${i}`));
    expect(set.size).toBeGreaterThan(40);
  });
});

describe("tileVars", () => {
  it("--h 변수 하나짜리 style 문자열을 돌려준다", () => {
    expect(tileVars(70)).toBe("--h: 70");
    expect(tileVars(0)).toBe("--h: 0");
    expect(tileVars(359)).toBe("--h: 359");
  });
});
