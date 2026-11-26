import { describe, expect, it } from "vitest";
import type { WatchStats } from "./watchStats";
import {
  evaluateHealth,
  formatStatsForAi,
  HISTORY_LIMIT,
  RECENT_WINDOW_MS,
  summarizeSeries,
  WatchStatsTracker,
} from "./watchStatsWindow";

function makeStats(over: Partial<WatchStats> = {}): WatchStats {
  return {
    width: 1920,
    height: 1080,
    fps: 60,
    codec: "H264",
    decoder: "ExternalDecoder, HW",
    videoKbps: 6840,
    audioKbps: 128,
    connectionRoute: "UDP · host→host",
    avOffsetMs: 12,
    videoJitterMs: 38,
    audioJitterMs: 42,
    rttMs: 3,
    framesDropped: 0,
    freezeCount: 0,
    framesDecoded: 600,
    audioPackets: 500,
    videoTargetMs: 150,
    audioTargetMs: 150,
    audioConcealedSamples: 0,
    videoPacketsLost: 0,
    videoPacketsReceived: 1000,
    nackCount: 0,
    pliCount: 0,
    firCount: 0,
    retransmittedPacketsReceived: 0,
    keyFramesDecoded: 2,
    framesReceived: 600,
    totalFreezesDurationMs: 0,
    pauseCount: 0,
    totalPausesDurationMs: 0,
    videoPacketsDiscarded: 0,
    videoNetJitterMs: 4,
    decodeMsPerFrame: 3.1,
    assemblyMsPerFrame: 1.5,
    audioPacketsLost: 0,
    audioPacketsDiscarded: 0,
    concealmentEvents: 0,
    silentConcealedSamples: 0,
    insertedSamplesForDeceleration: 0,
    removedSamplesForAcceleration: 0,
    audioNetJitterMs: 2,
    totalSamplesReceived: 480_000,
    audioCodec: "opus",
    ...over,
  };
}

describe("WatchStatsTracker: 최근 10초 구간 및 시계열", () => {
  it("첫 샘플은 기준만 잡고, 이후 10초 창의 증분·손실률·은닉 ms를 구한다", () => {
    const tracker = new WatchStatsTracker();
    const s1 = tracker.push({ now: 1000, stats: makeStats(), bufferMs: 150, targetFps: 60 });
    expect(s1.recent.spanSec).toBe(0);
    expect(s1.recent.videoLossRatePct).toBeNull();
    expect(s1.health).toEqual({ tone: "ok", label: "정상", reasons: [] });

    const s2 = tracker.push({
      now: 6000,
      stats: makeStats({
        videoPacketsReceived: 1990,
        videoPacketsLost: 10,
        nackCount: 10,
        retransmittedPacketsReceived: 8,
        audioConcealedSamples: 48 * 30, // 30ms
      }),
      bufferMs: 150,
      targetFps: 60,
    });
    expect(s2.recent.spanSec).toBe(5);
    expect(s2.recent.videoPacketsLost).toBe(10);
    expect(s2.recent.videoPacketsReceived).toBe(990);
    expect(s2.recent.videoLossRatePct).toBeCloseTo(1.0);
    expect(s2.recent.videoUnrecoveredLost).toBe(2);
    expect(s2.recent.audioConcealMs).toBeCloseTo(30);
    expect(s2.health.tone).toBe("warn");
    expect(s2.health.label).toBe("패킷 복구 중");

    // 10초가 지나 예전 손실 구간이 창 밖으로 밀려나면 정상으로 돌아온다
    const s3 = tracker.push({
      now: 6000 + RECENT_WINDOW_MS + 1000,
      stats: makeStats({
        videoPacketsReceived: 3990,
        videoPacketsLost: 10,
        nackCount: 10,
        retransmittedPacketsReceived: 8,
        audioConcealedSamples: 48 * 30,
      }),
      bufferMs: 150,
      targetFps: 60,
    });
    expect(s3.recent.videoPacketsLost).toBe(0);
    expect(s3.recent.videoLossRatePct).toBe(0);
    expect(s3.recent.audioConcealMs).toBe(0);
    expect(s3.health).toEqual({ tone: "ok", label: "정상", reasons: [] });
  });

  it("카운터가 줄어들어도(디코더 재생성·화질 전환) 음수 증분이 나지 않는다", () => {
    const tracker = new WatchStatsTracker();
    tracker.push({ now: 1000, stats: makeStats({ videoPacketsLost: 50, freezeCount: 3 }), bufferMs: 150 });
    const next = tracker.push({
      now: 2000,
      stats: makeStats({ videoPacketsLost: 2, freezeCount: 0, videoPacketsReceived: 500 }),
      bufferMs: 150,
    });
    expect(next.recent.videoPacketsLost).toBe(0);
    expect(next.recent.freezes).toBe(0);
  });

  it("시계열 링 버퍼는 HISTORY_LIMIT(60)개를 넘지 않는다", () => {
    const tracker = new WatchStatsTracker();
    let snap = tracker.push({ now: 0, stats: makeStats({ videoKbps: 1000, rttMs: 2 }), bufferMs: 150 });
    for (let i = 1; i <= HISTORY_LIMIT + 15; i++) {
      snap = tracker.push({
        now: i * 1000,
        stats: makeStats({ videoKbps: 1000 + i * 100, rttMs: 2 + (i % 5) }),
        bufferMs: 150,
      });
    }
    expect(snap.history.videoMbps).toHaveLength(HISTORY_LIMIT);
    expect(snap.history.rttMs).toHaveLength(HISTORY_LIMIT);
    expect(snap.history.videoMbps[0]).toBeCloseTo(2.6);
  });
});

describe("evaluateHealth", () => {
  const cleanRecent = {
    spanSec: 10,
    videoPacketsLost: 0,
    videoPacketsReceived: 1000,
    videoLossRatePct: 0,
    videoUnrecoveredLost: 0,
    nackCount: 0,
    retransmittedPackets: 0,
    pliCount: 0,
    freezes: 0,
    freezeMs: 0,
    framesDropped: 0,
    keyframeRequests: 0,
    audioPacketsLost: 0,
    audioConcealMs: 0,
    audioConcealEvents: 0,
  };

  it("화면 멈춤·소리 끊김·미복구 손실은 err", () => {
    expect(
      evaluateHealth({ stats: makeStats(), recent: { ...cleanRecent, freezes: 1, freezeMs: 400 }, bufferMs: 150 }),
    ).toMatchObject({ tone: "err", label: "화면 멈춤" });
    expect(
      evaluateHealth({ stats: makeStats(), recent: { ...cleanRecent, audioConcealMs: 60 }, bufferMs: 150 }),
    ).toMatchObject({ tone: "err", label: "소리 끊김" });
    expect(
      evaluateHealth({
        stats: makeStats(),
        recent: { ...cleanRecent, videoPacketsLost: 30, videoLossRatePct: 3, videoUnrecoveredLost: 15 },
        bufferMs: 150,
      }),
    ).toMatchObject({ tone: "err", label: "패킷 손실" });
  });

  it("버퍼 확장·디코딩 부담·A/V 어긋남은 warn", () => {
    expect(evaluateHealth({ stats: makeStats(), recent: cleanRecent, bufferMs: 220 })).toMatchObject({
      tone: "warn",
      label: "버퍼 확장 중",
    });
    expect(
      evaluateHealth({ stats: makeStats({ decodeMsPerFrame: 14 }), recent: cleanRecent, bufferMs: 150, targetFps: 60 }),
    ).toMatchObject({ tone: "warn", label: "디코딩 부담" });
    expect(
      evaluateHealth({ stats: makeStats({ avOffsetMs: -85 }), recent: cleanRecent, bufferMs: 150 }),
    ).toMatchObject({ tone: "warn", label: "A/V 어긋남" });
  });
});

describe("summarizeSeries & formatStatsForAi", () => {
  it("summarizeSeries는 null을 빼고 min/avg/max를 구한다", () => {
    expect(summarizeSeries([null, null])).toBeNull();
    const s = summarizeSeries([null, 2, 4, 6]);
    expect(s).toEqual({ min: 2, avg: 4, max: 6 });
  });

  it("formatStatsForAi는 AI가 바로 해석할 수 있는 섹션과 단위를 포함한다", () => {
    const tracker = new WatchStatsTracker();
    tracker.push({ now: 1000, stats: makeStats({ videoKbps: 6000, rttMs: 2 }), bufferMs: 150, targetFps: 60 });
    const snap = tracker.push({
      now: 6000,
      stats: makeStats({
        videoKbps: 6840,
        rttMs: 4,
        videoPacketsReceived: 2000,
        videoPacketsLost: 5,
        nackCount: 5,
        retransmittedPacketsReceived: 5,
      }),
      bufferMs: 210,
      targetFps: 60,
    });
    const text = formatStatsForAi({
      stats: makeStats({ videoKbps: 6840, rttMs: 4 }),
      snapshot: snap,
      bufferMs: 210,
      audioRed: true,
      streamIds: ["video:s1", "audio:s1"],
      broadcasterName: "하린",
      serverName: "우리집 서버",
      targetWidth: 1920,
      targetHeight: 1080,
      targetFps: 60,
      capturedAt: "2026-11-27T01:55:00.000Z",
    });

    expect(text).toContain("[dongsi watch diagnostics]");
    expect(text).toContain('stream: broadcaster="하린" server="우리집 서버" target=1920x1080@60fps audio_red=true route="UDP · host→host"');
    expect(text).toContain('health: status=warn label="버퍼 확장 중"');
    expect(text).toContain("buffer_expanded(210ms>150ms)");
    expect(text).toContain("[current]");
    expect(text).toContain(
      "timing: viewer_latency_ms=43 av_offset_ms=+12 rtt_ms=4.0 video_jb_ms=38 audio_jb_ms=42 playout_buffer_ms=210 (base=150)",
    );
    expect(text).toContain("[recent_10s] (span=5.0s)");
    expect(text).toContain("loss_pct=0.50% lost=5 recv=1000 unrecovered_est=0 nack=5 rtx=5");
    expect(text).toContain("[trend_60s] (samples=2)");
    expect(text).toContain("video_mbps: min=6.00 avg=6.42 max=6.84");
    expect(text).toContain("[cumulative]");
    expect(text).toContain("streams: video:s1 audio:s1");
  });
});
