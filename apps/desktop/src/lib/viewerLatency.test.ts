import { describe, expect, it } from "vitest";
import { viewerLatencyMs } from "./viewerLatency";

const input = { rttMs: 4, videoJitterMs: 38, decodeMsPerFrame: 3 };

describe("viewerLatencyMs", () => {
  it("RTT/2 + 비디오 지터 버퍼 + 디코딩", () => {
    expect(viewerLatencyMs(input)).toBe(43);
    expect(viewerLatencyMs({ rttMs: 0, videoJitterMs: 0, decodeMsPerFrame: 0 })).toBe(0);
  });

  it("구성 값 중 하나라도 없으면 null", () => {
    expect(viewerLatencyMs({ ...input, rttMs: null })).toBeNull();
    expect(viewerLatencyMs({ ...input, videoJitterMs: null })).toBeNull();
    expect(viewerLatencyMs({ ...input, decodeMsPerFrame: null })).toBeNull();
  });

  it("합한 뒤 한 번만 반올림한다", () => {
    // 항마다 반올림하면 0 + 38 + 3 = 41이지만, 합 0.4 + 38.4 + 3.4 = 42.2 → 42
    expect(viewerLatencyMs({ rttMs: 0.8, videoJitterMs: 38.4, decodeMsPerFrame: 3.4 })).toBe(42);
    expect(viewerLatencyMs({ rttMs: 3, videoJitterMs: 38, decodeMsPerFrame: 3.1 })).toBe(43);
  });
});
