import { describe, expect, it } from "vitest";
import {
  MAX_BITRATE_KBPS,
  MAX_BITRATE_MBPS,
  MIN_BITRATE_KBPS,
  MIN_BITRATE_MBPS,
  fmtMbps,
  kbpsToMbps,
  mbpsToKbps,
  normalizeKbps,
} from "./bitrate";
import { bitrateZones } from "./bitrateZones";

describe("비트레이트 단위(Mbps 표시, kbps 저장)", () => {
  it("kbpsToMbps", () => {
    expect(kbpsToMbps(7000)).toBe(7);
    expect(kbpsToMbps(4900)).toBe(4.9);
    expect(kbpsToMbps(1750)).toBe(1.75);
    expect(kbpsToMbps(6955)).toBe(6.96);
    expect(kbpsToMbps(300)).toBe(0.3);
    expect(kbpsToMbps(100_000)).toBe(100);
    expect(kbpsToMbps(null)).toBeNull();
    expect(kbpsToMbps(undefined)).toBeNull();
    expect(kbpsToMbps(Number.NaN)).toBeNull();
  });
  it("mbpsToKbps는 소수 둘째 자리(10 kbps)로", () => {
    expect(mbpsToKbps(7)).toBe(7000);
    expect(mbpsToKbps(12.5)).toBe(12_500);
    expect(mbpsToKbps(7.25)).toBe(7250);
    expect(mbpsToKbps(0.3)).toBe(300);
    expect(mbpsToKbps(100)).toBe(100_000);
    expect(mbpsToKbps(7.257)).toBe(7260);
    expect(mbpsToKbps(7.252)).toBe(7250);
    expect(mbpsToKbps(null)).toBeNull();
    expect(mbpsToKbps(Number.NaN)).toBeNull();
  });
  it("구간 경계·기본값(kbps)은 Mbps로 바꿔도 그대로 돌아온다", () => {
    for (const [width, height] of [[1920, 1080], [2560, 1440], [3840, 2160], [1720, 1000]]) {
      for (const fps of [30, 60, 144]) {
        const z = bitrateZones({ width, height, fps });
        for (const kbps of [z.low, z.recommended, z.high, z.barMin, z.barMax]) {
          expect(mbpsToKbps(kbpsToMbps(kbps))).toBe(kbps);
        }
      }
    }
  });
  it("0.01 Mbps 단위는 모두 그대로 돌아온다", () => {
    for (let k = 30; k <= 10_000; k++) {
      expect(mbpsToKbps(kbpsToMbps(k * 10))).toBe(k * 10);
    }
  });
  it("범위는 Rust VideoSettings와 같다", () => {
    expect(mbpsToKbps(MIN_BITRATE_MBPS)).toBe(300);
    expect(mbpsToKbps(MAX_BITRATE_MBPS)).toBe(25_000);
    expect(MIN_BITRATE_KBPS).toBe(300);
    expect(MAX_BITRATE_KBPS).toBe(25_000);
  });
  it("normalizeKbps: 비었거나 0 이하면 기본값, 10 kbps 단위, 범위 안", () => {
    expect(normalizeKbps(7000.4, 1)).toBe(7000);
    expect(normalizeKbps(7256, 1)).toBe(7260);
    expect(normalizeKbps(null, 5000)).toBe(5000);
    expect(normalizeKbps(Number.NaN, 5000)).toBe(5000);
    expect(normalizeKbps(0, 5000)).toBe(5000);
    expect(normalizeKbps(-3, 5000)).toBe(5000);
    expect(normalizeKbps(100, 5000)).toBe(300);
    expect(normalizeKbps(30_000, 5000)).toBe(25_000);
  });
  it("fmtMbps", () => {
    expect(fmtMbps(6950)).toBe("6.95 Mbps");
    expect(fmtMbps(7000)).toBe("7.00 Mbps");
    expect(fmtMbps(0)).toBe("0.00 Mbps");
    expect(fmtMbps(null)).toBe("– Mbps");
  });
});
