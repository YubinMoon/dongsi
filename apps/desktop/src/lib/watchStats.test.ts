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

describe("collectStats: 진단용 값", () => {
  const video = {
    id: "V",
    type: "inbound-rtp",
    kind: "video",
    timestamp: 2000,
    codecId: "CV",
    packetsLost: 4054,
    packetsReceived: 90000,
    nackCount: 812,
    pliCount: 9,
    firCount: 0,
    retransmittedPacketsReceived: 640,
    keyFramesDecoded: 15,
    framesReceived: 3000,
    framesDecoded: 2990,
    totalFreezesDuration: 31.2,
    pauseCount: 1,
    totalPausesDuration: 2.5,
    packetsDiscarded: 3,
    jitter: 0.0125,
    totalDecodeTime: 9.3,
    framesAssembledFromMultiplePackets: 2000,
    totalAssemblyTime: 4,
  };
  const audio = {
    id: "A",
    type: "inbound-rtp",
    kind: "audio",
    timestamp: 2000,
    codecId: "CA",
    packetsLost: 12,
    packetsDiscarded: 4,
    concealmentEvents: 37,
    silentConcealedSamples: 960,
    insertedSamplesForDeceleration: 100,
    removedSamplesForAcceleration: 200,
    jitter: 0.004,
    totalSamplesReceived: 480000,
  };

  it("누적 값은 그대로, 초 단위 값은 ms로", () => {
    const { stats } = collectStats(
      report([video, audio, { id: "CA", type: "codec", mimeType: "audio/opus" }]),
      null,
    );
    expect(stats.videoPacketsLost).toBe(4054);
    expect(stats.videoPacketsReceived).toBe(90000);
    expect(stats.nackCount).toBe(812);
    expect(stats.pliCount).toBe(9);
    expect(stats.firCount).toBe(0);
    expect(stats.retransmittedPacketsReceived).toBe(640);
    expect(stats.keyFramesDecoded).toBe(15);
    expect(stats.framesReceived).toBe(3000);
    expect(stats.totalFreezesDurationMs).toBeCloseTo(31200);
    expect(stats.pauseCount).toBe(1);
    expect(stats.totalPausesDurationMs).toBeCloseTo(2500);
    expect(stats.videoPacketsDiscarded).toBe(3);
    expect(stats.videoNetJitterMs).toBeCloseTo(12.5);
    expect(stats.audioPacketsLost).toBe(12);
    expect(stats.audioPacketsDiscarded).toBe(4);
    expect(stats.concealmentEvents).toBe(37);
    expect(stats.silentConcealedSamples).toBe(960);
    expect(stats.insertedSamplesForDeceleration).toBe(100);
    expect(stats.removedSamplesForAcceleration).toBe(200);
    expect(stats.audioNetJitterMs).toBeCloseTo(4);
    expect(stats.totalSamplesReceived).toBe(480000);
    expect(stats.audioCodec).toBe("opus");
    // 기존 합계 필드는 그대로
    expect(stats.packetsLost).toBe(4066);
  });

  it("오디오 코덱: red도 그대로", () => {
    const { stats } = collectStats(report([audio, { id: "CA", type: "codec", mimeType: "audio/red" }]), null);
    expect(stats.audioCodec).toBe("red");
  });

  it("없는 값은 0이 아니라 null", () => {
    const { stats } = collectStats(report([{ id: "V", type: "inbound-rtp", kind: "video", timestamp: 1000 }]), null);
    for (const k of [
      "videoPacketsLost",
      "nackCount",
      "pliCount",
      "retransmittedPacketsReceived",
      "keyFramesDecoded",
      "totalFreezesDurationMs",
      "videoNetJitterMs",
      "decodeMsPerFrame",
      "assemblyMsPerFrame",
      "audioPacketsLost",
      "concealmentEvents",
      "audioNetJitterMs",
      "audioCodec",
    ] as const) {
      expect(stats[k], k).toBeNull();
    }
    // 숫자가 아닌 값도 null
    const bad = collectStats(
      report([{ id: "V", type: "inbound-rtp", kind: "video", timestamp: 1000, nackCount: "7", pliCount: NaN }]),
      null,
    );
    expect(bad.stats.nackCount).toBeNull();
    expect(bad.stats.pliCount).toBeNull();
  });

  it("구간 값: 디코딩·조립 시간은 직전 측정과의 차로", () => {
    const first = collectStats(report([video]), null);
    const next = {
      ...video,
      timestamp: 3000,
      framesDecoded: 2990 + 60,
      totalDecodeTime: 9.3 + 0.186,
      framesAssembledFromMultiplePackets: 2000 + 40,
      totalAssemblyTime: 4 + 0.08,
    };
    const { stats } = collectStats(report([next]), first.current);
    expect(stats.decodeMsPerFrame).toBeCloseTo(3.1);
    expect(stats.assemblyMsPerFrame).toBeCloseTo(2);
  });

  it("구간에 디코딩이 없으면 decodeMsPerFrame은 null", () => {
    const first = collectStats(report([video]), null);
    const { stats } = collectStats(report([{ ...video, timestamp: 3000 }]), first.current);
    expect(stats.decodeMsPerFrame).toBeNull();
    expect(stats.assemblyMsPerFrame).toBeNull();
  });
});
