import { describe, expect, it } from "vitest";
import { WatchLogger, type WatchLogInput } from "./watchLog";
import type { WatchStats } from "./watchStats";

function stats(over: Partial<WatchStats> = {}): WatchStats {
  return {
    width: 1920,
    height: 1080,
    fps: 60,
    codec: "H264",
    decoder: "ExternalDecoder, HW",
    videoKbps: 6000,
    audioKbps: 128,
    latencyMs: 84,
    avOffsetMs: 12,
    videoJitterMs: 38,
    audioJitterMs: 42,
    rttMs: 3,
    framesDropped: 0,
    freezeCount: 0,
    framesDecoded: 100,
    audioPackets: 250,
    videoTargetMs: null,
    audioTargetMs: null,
    audioConcealedSamples: 0,
    videoPacketsLost: 0,
    videoPacketsReceived: 1000,
    nackCount: 0,
    pliCount: 0,
    firCount: 0,
    retransmittedPacketsReceived: 0,
    keyFramesDecoded: 1,
    framesReceived: 100,
    totalFreezesDurationMs: 0,
    pauseCount: 0,
    totalPausesDurationMs: 0,
    videoPacketsDiscarded: 0,
    videoNetJitterMs: 5,
    decodeMsPerFrame: 3.1,
    assemblyMsPerFrame: 1,
    audioPacketsLost: 0,
    audioPacketsDiscarded: 0,
    concealmentEvents: 0,
    silentConcealedSamples: 0,
    insertedSamplesForDeceleration: 0,
    removedSamplesForAcceleration: 0,
    audioNetJitterMs: 2,
    totalSamplesReceived: 48000,
    audioCodec: "opus",
    ...over,
  };
}

function input(now: number, s: Partial<WatchStats> = {}, over: Partial<WatchLogInput> = {}): WatchLogInput {
  return { now, stats: stats(s), bufferMs: 100, hidden: false, keyframeRequested: false, audioRed: false, ...over };
}

/** 첫 통계(기준 잡기)를 넣고 시작한다. 첫 통계는 줄을 내지 않는다. */
function started(opts?: ConstructorParameters<typeof WatchLogger>[0]) {
  const log = new WatchLogger(opts);
  const first = log.update(input(0));
  return { log, first };
}

describe("WatchLogger 요약", () => {
  it("첫 통계는 기준만 잡는다(요약·사건 없음)", () => {
    const { first } = started();
    expect(first).toEqual([]);
  });

  it("summaryMs마다 요약 한 줄: 구간 증가량·평균·지금 값", () => {
    const { log } = started();
    expect(log.update(input(1000, { videoPacketsLost: 10, fps: 50, videoKbps: 5000 }))).toEqual([]);
    expect(log.update(input(5000, { videoPacketsLost: 40, fps: 30, videoKbps: 7000, nackCount: 30 }))).toEqual([]);
    const lines = log.update(
      input(10_000, {
        videoPacketsLost: 100,
        audioPacketsLost: 3,
        nackCount: 80,
        retransmittedPacketsReceived: 60,
        pliCount: 2,
        keyFramesDecoded: 3,
        framesDropped: 7,
        audioPacketsDiscarded: 1,
        fps: 40,
        videoKbps: 6000,
        latencyMs: 16433,
      }),
    );
    expect(lines).toHaveLength(1);
    const line = lines[0];
    expect(line.startsWith("watch-sum ")).toBe(true);
    expect(line).toContain("win_s=10.0");
    expect(line).toContain(" lost=100/3 ");
    expect(line).toContain(" nack=80 rtx=60 pli=2 key=2 freeze=0/0ms ");
    expect(line).toContain(" dropped=7 conceal_ms=0 a_discard=1 suppressed=0 ");
    // fps는 통계마다 값의 평균: (60+50+30+40)/4, kbps는 첫 통계를 빼고 평균: (5000+7000+6000)/3
    expect(line).toContain(" fps=45.0 kbps=6000+128 ");
    expect(line).toContain(' res=1920x1080 codec=H264 decoder="ExternalDecoder, HW" dec_ms=3.1 ');
    expect(line).toContain(" jb=38/42 latency_clock=16433 rtt=3 buf=100 ");
    expect(line).toContain(" buf=100 hidden=false red=0");
    expect(line.length).toBeLessThanOrEqual(600);
    // 예시 출력(보고용)
    console.info(line);
  });

  it("그 뒤 요약은 다음 구간의 증가량만(누적이 아니라 구간)", () => {
    const { log } = started();
    log.update(input(10_000, { videoPacketsLost: 100 }));
    expect(log.update(input(15_000, { videoPacketsLost: 130 }))).toEqual([]);
    const lines = log.update(input(20_000, { videoPacketsLost: 150 }));
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain(" lost=50/0 ");
    expect(lines[0]).toContain("win_s=10.0");
  });

  it("summaryMs 옵션", () => {
    const { log } = started({ summaryMs: 2000 });
    expect(log.update(input(1000))).toEqual([]);
    expect(log.update(input(2000)).filter((l) => l.startsWith("watch-sum "))).toHaveLength(1);
  });

  it("null 필드는 -", () => {
    const log = new WatchLogger();
    const nulls: Partial<WatchStats> = {
      fps: null,
      codec: null,
      decoder: null,
      latencyMs: null,
      rttMs: null,
      videoJitterMs: null,
      audioJitterMs: null,
      decodeMsPerFrame: null,
      videoPacketsLost: null,
      audioPacketsLost: null,
      nackCount: null,
      retransmittedPacketsReceived: null,
      pliCount: null,
      keyFramesDecoded: null,
      totalFreezesDurationMs: null,
      audioPacketsDiscarded: null,
    };
    log.update(input(0, nulls, { bufferMs: null }));
    const lines = log.update(input(10_000, nulls, { bufferMs: null }));
    expect(lines).toHaveLength(1);
    const line = lines[0];
    expect(line).toContain(" lost=-/- nack=- rtx=- pli=- key=- ");
    expect(line).toContain(" freeze=0/-ms ");
    expect(line).toContain(" fps=- ");
    expect(line).toContain(" res=1920x1080 codec=- decoder=- dec_ms=- jb=-/- latency_clock=- rtt=- buf=- hidden=false");
  });
});

describe("WatchLogger 사건", () => {
  it("freeze: 멈춤 수·시간과 그 구간의 손실·복구 증가", () => {
    const { log } = started();
    log.update(input(1000));
    const lines = log.update(
      input(2000, {
        freezeCount: 1,
        totalFreezesDurationMs: 1200,
        videoPacketsLost: 40,
        audioPacketsLost: 2,
        nackCount: 30,
        retransmittedPacketsReceived: 20,
        pliCount: 1,
        fps: 12,
      }),
    );
    expect(lines).toEqual(["watch-event freeze n=1 dur_ms=1200 lost_v=40 lost_a=2 nack=30 rtx=20 pli=1 fps=12"]);
    console.info(lines[0]);
    // 늘지 않으면 다시 나오지 않는다
    expect(log.update(input(3000, { freezeCount: 1, totalFreezesDurationMs: 1200 }))).toEqual([]);
  });

  it("keyframe-request: 앱이 키프레임을 요청했을 때", () => {
    const { log } = started();
    const lines = log.update(input(1000, { framesDecoded: 100, videoKbps: 5000 }, { keyframeRequested: true }));
    expect(lines).toEqual(["watch-event keyframe-request frames_decoded=100 kbps=5000 pli=0"]);
    console.info(lines[0]);
    expect(log.update(input(2000))).toEqual([]);
  });

  it("audio-gap: 은닉 50ms(2400샘플) 이상만, 손실로 설명되는 몫과 버림", () => {
    const { log } = started();
    // 2399샘플: 50ms 미만
    expect(log.update(input(1000, { audioConcealedSamples: 2399 }))).toEqual([]);
    // 2399 → 7199: 4800샘플 = 100ms. 같은 구간 오디오 손실 5개 = 50ms
    const lines = log.update(
      input(2000, { audioConcealedSamples: 7199, audioPacketsLost: 5, audioPacketsDiscarded: 2 }),
    );
    expect(lines).toEqual(["watch-event audio-gap concealed_ms=100 lost_ms=50 discarded=2 red=0"]);
    console.info(lines[0]);
    // 정확히 2400샘플은 사건
    expect(log.update(input(3000, { audioConcealedSamples: 9599, audioPacketsLost: 5, audioPacketsDiscarded: 2 }))).toHaveLength(1);
  });

  it("audioRed: 요약과 audio-gap에 red=1", () => {
    const { log } = started();
    const lines = log.update(
      input(10_000, { audioConcealedSamples: 4800, audioPacketsLost: 5, audioPacketsDiscarded: 1950 }, { audioRed: true }),
    );
    expect(lines).toHaveLength(2);
    expect(lines[0]).toBe("watch-event audio-gap concealed_ms=100 lost_ms=50 discarded=1950 red=1");
    expect(lines[1]).toContain(" a_discard=1950 ");
    expect(lines[1].endsWith(" hidden=false red=1")).toBe(true);
  });

  it("buffer: 값이 바뀔 때만 from→to(처음 값은 기준)", () => {
    const { log } = started();
    expect(log.update(input(1000, {}, { bufferMs: 100 }))).toEqual([]);
    expect(log.update(input(2000, {}, { bufferMs: 145 }))).toEqual(["watch-event buffer from=100 to=145"]);
    expect(log.update(input(3000, {}, { bufferMs: 145 }))).toEqual([]);
    expect(log.update(input(4000, {}, { bufferMs: null }))).toEqual(["watch-event buffer from=145 to=-"]);
  });

  it("hidden: 상태가 바뀔 때(처음부터 숨김이면 한 번)", () => {
    const { log } = started();
    expect(log.update(input(1000, {}, { hidden: true }))).toEqual(["watch-event hidden hidden=true"]);
    expect(log.update(input(2000, {}, { hidden: true }))).toEqual([]);
    expect(log.update(input(3000, {}, { hidden: false }))).toEqual(["watch-event hidden hidden=false"]);
    const hiddenAtStart = new WatchLogger();
    expect(hiddenAtStart.update(input(0, {}, { hidden: true }))).toEqual(["watch-event hidden hidden=true"]);
  });
});

describe("WatchLogger 사건 상한", () => {
  it("한 구간에 maxEventsPerWindow개까지, 넘친 수는 요약에 suppressed=N", () => {
    const { log } = started({ maxEventsPerWindow: 3 });
    const events: string[] = [];
    let count = 0;
    for (let i = 1; i <= 8; i++) {
      count += 1;
      events.push(...log.update(input(i * 1000, { freezeCount: count })));
    }
    expect(events).toHaveLength(3);
    expect(events.every((l) => l.startsWith("watch-event freeze "))).toBe(true);
    const lines = log.update(input(10_000, { freezeCount: count + 1 }));
    expect(lines).toHaveLength(1);
    // 멈춤 사건 9번 중 3개만 나왔다. 6개를 눌렀다.
    expect(lines[0]).toContain(" suppressed=6 ");
    expect(lines[0]).toContain(" freeze=9/");
    // 다음 구간은 다시 낸다
    const next = log.update(input(11_000, { freezeCount: count + 2 }));
    expect(next).toHaveLength(1);
    expect(next[0].startsWith("watch-event freeze ")).toBe(true);
    const after = log.update(input(20_000, { freezeCount: count + 2 }));
    expect(after[0]).toContain(" suppressed=0 ");
  });
});

describe("WatchLogger 카운터 되감김·reset", () => {
  it("카운터가 줄면 그 구간의 증가량은 0, 기준을 새로 잡는다", () => {
    const { log } = started();
    log.update(input(1000, { videoPacketsLost: 500, freezeCount: 3, nackCount: 100, audioConcealedSamples: 96000 }));
    // 새 연결: 누적이 0으로 돌아왔다. 멈춤·은닉 사건은 나오지 않는다.
    expect(log.update(input(2000, { videoPacketsLost: 2, freezeCount: 0, nackCount: 1, audioConcealedSamples: 0 }))).toEqual([]);
    const lines = log.update(input(10_000, { videoPacketsLost: 12, freezeCount: 0, nackCount: 5, audioConcealedSamples: 0 }));
    expect(lines).toHaveLength(1);
    // 500(첫 구간 일부) 증가 + 되감긴 구간 0 + 10 증가
    expect(lines[0]).toContain(" lost=510/0 nack=104 ");
    expect(lines[0]).toContain(" conceal_ms=2000 ");
    expect(lines[0]).toContain(" freeze=3/0ms ");
  });

  it("reset하면 기준과 구간을 모두 비운다", () => {
    const { log } = started();
    log.update(input(1000, { freezeCount: 2 }));
    log.reset();
    // 첫 통계가 다시 기준이 된다(멈춤 사건·요약 없음)
    expect(log.update(input(20_000, { freezeCount: 5 }))).toEqual([]);
    expect(log.update(input(21_000, { freezeCount: 5 }))).toEqual([]);
    expect(log.update(input(30_000, { freezeCount: 5 }))[0]).toContain(" freeze=0/0ms ");
  });
});

describe("WatchLogger 줄 길이", () => {
  it("긴 값이 들어와도 한 줄 600자 안쪽", () => {
    const log = new WatchLogger();
    const long = "x".repeat(800);
    const lines = [
      ...log.update(input(0, { decoder: long, codec: long })),
      ...log.update(input(10_000, { decoder: long, codec: long })),
    ];
    expect(lines.length).toBeGreaterThan(0);
    for (const l of lines) expect(l.length).toBeLessThanOrEqual(600);
  });
});
