import { describe, expect, it } from "vitest";
import { collectStats } from "./watchStats";

/** getStats() 보고서를 흉내 낸다(Map은 forEach·get이 보고서와 같은 모양이다). */
function report(entries: Record<string, unknown>[]): RTCStatsReport {
  return new Map(entries.map((e) => [String(e.id), e])) as unknown as RTCStatsReport;
}

describe("collectStats: 오디오 은닉 샘플", () => {
  it("오디오 inbound-rtp의 concealedSamples 누적 값을 그대로 낸다", () => {
    const { stats } = collectStats(
      report([
        { id: "V", type: "inbound-rtp", kind: "video", timestamp: 1000, concealedSamples: 99 },
        { id: "A", type: "inbound-rtp", kind: "audio", timestamp: 1000, concealedSamples: 7440 },
      ]),
      null,
    );
    expect(stats.audioConcealedSamples).toBe(7440); // 비디오의 값이 아니라 오디오의 값
  });

  it("오디오가 없거나 값이 없으면 0", () => {
    const noAudio = collectStats(
      report([{ id: "V", type: "inbound-rtp", kind: "video", timestamp: 1000 }]),
      null,
    );
    expect(noAudio.stats.audioConcealedSamples).toBe(0);
    const noField = collectStats(
      report([{ id: "A", type: "inbound-rtp", kind: "audio", timestamp: 1000 }]),
      null,
    );
    expect(noField.stats.audioConcealedSamples).toBe(0);
  });
});
