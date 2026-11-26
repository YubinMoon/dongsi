import { describe, expect, it } from "vitest";
import {
  backHref,
  cardFailureMessage,
  checkFrozen,
  checkStreamChange,
  classifyStartError,
  compactStatRows,
  detailedStatSections,
  devLogLine,
  effectiveMuted,
  endedDescription,
  FREEZE_AFTER_MS,
  FREEZE_INITIAL,
  isPopoutHash,
  isTypingTarget,
  KEYFRAME_RETRY_MS,
  MEDIA_FAILED_MESSAGE,
  muteButtonLabel,
  passwordLabel,
  passwordSubtitle,
  shouldResume,
  showInfoBar,
  showLive,
  sparklineGeometry,
  statsIntervalMs,
  statusFromConnectionState,
  STATS_FAST_PHASE_MS,
  STREAM_SIG_INITIAL,
  toggleMute,
  viewersLabel,
  volumeIconName,
} from "./watch";
import type { WatchStats } from "./watchStats";
import { WatchStatsTracker } from "./watchStatsWindow";

function stats(over: Partial<WatchStats> = {}): WatchStats {
  return {
    width: 1920,
    height: 1080,
    fps: 60,
    codec: "H264",
    decoder: null,
    videoKbps: 6842,
    audioKbps: 128,
    connectionRoute: "UDP · host→host",
    latencyMs: 84,
    avOffsetMs: 12,
    videoJitterMs: 38,
    audioJitterMs: 42,
    rttMs: 3,
    framesDropped: 234,
    freezeCount: 42,
    framesDecoded: 100,
    audioPackets: 250,
    videoTargetMs: null,
    audioTargetMs: null,
    audioConcealedSamples: 0,
    videoPacketsLost: 4054,
    videoPacketsReceived: 90000,
    nackCount: 812,
    pliCount: 9,
    firCount: 0,
    retransmittedPacketsReceived: 640,
    keyFramesDecoded: 15,
    framesReceived: 3000,
    totalFreezesDurationMs: 31200,
    pauseCount: 0,
    totalPausesDurationMs: 0,
    videoPacketsDiscarded: 0,
    videoNetJitterMs: 12.5,
    decodeMsPerFrame: 3.14,
    assemblyMsPerFrame: 2,
    audioPacketsLost: 12,
    audioPacketsDiscarded: 4,
    concealmentEvents: 37,
    silentConcealedSamples: 0,
    insertedSamplesForDeceleration: 100,
    removedSamplesForAcceleration: 200,
    audioNetJitterMs: 4,
    totalSamplesReceived: 480000,
    audioCodec: "opus",
    ...over,
  };
}

describe("backHref·isPopoutHash", () => {
  it("나가기 대상은 0.1.0의 돌아가기와 같다", () => {
    expect(backHref("host")).toBe("#/broadcast");
    expect(backHref("local-abc")).toBe("#/");
    expect(backHref("srv-1")).toBe("#/server/srv-1");
  });
  it("분리 창은 해시의 popout=1로 안다", () => {
    expect(isPopoutHash("#/watch/a/b?popout=1")).toBe(true);
    expect(isPopoutHash("#/watch/a/b?x=1&popout=1")).toBe(true);
    expect(isPopoutHash("#/watch/a/b?popout=1&x=1")).toBe(true);
    expect(isPopoutHash("#/watch/a/b")).toBe(false);
    expect(isPopoutHash("#/watch/a/b?popout=10")).toBe(false);
  });
});

describe("표시 문자열", () => {
  it("viewersLabel", () => {
    expect(viewersLabel(3)).toBe("시청자 3명");
  });
  it("passwordSubtitle: 이름 · 세로 해상도 · fps", () => {
    expect(passwordSubtitle("도윤", { height: 1440, fps: 30 })).toBe("도윤 · 1440p · 30fps");
    expect(passwordSubtitle("하린", { height: 1080, fps: 60 })).toBe("하린 · 1080p · 60fps");
    expect(passwordSubtitle("하린", null)).toBe("하린");
  });
  it("endedDescription: 조사를 받침에 맞춘다", () => {
    expect(endedDescription("하린")).toBe("하린이 다시 방송을 시작하면 자동으로 다시 연결합니다.");
    expect(endedDescription("민수")).toBe("민수가 다시 방송을 시작하면 자동으로 다시 연결합니다.");
    expect(endedDescription("")).toBe("방송자가 다시 방송을 시작하면 자동으로 다시 연결합니다.");
  });
  it("passwordLabel", () => {
    expect(passwordLabel("도윤")).toBe("'도윤' 방송의 비밀번호");
  });
  it("cardFailureMessage", () => {
    expect(cardFailureMessage({ kind: "failed", message: "연결 시간이 지났습니다" })).toBe("연결 시간이 지났습니다");
    expect(
      cardFailureMessage({
        kind: "failed",
        message: "127.0.0.1:8471에 접속하지 못했습니다: 연결을 거부했으므로 연결하지 못했습니다. (os error 10061)",
      }),
    ).toBe("127.0.0.1:8471에 접속하지 못했습니다: 연결을 거부했으므로 연결하지 못했습니다.");
    expect(cardFailureMessage({ kind: "connecting" })).toBe("서버에 연결하지 못했습니다.");
    expect(cardFailureMessage(undefined)).toBe("서버에 연결하지 못했습니다.");
  });
});

describe("classifyStartError 접속 실패 문장", () => {
  it("오류 화면 문구에서 os error 번호를 뺀다", () => {
    expect(
      classifyStartError(
        { code: null, message: "127.0.0.1:8471에 접속하지 못했습니다: 연결을 거부했으므로 연결하지 못했습니다. (os error 10061)" },
        false,
      ),
    ).toEqual({
      status: "error",
      message: "127.0.0.1:8471에 접속하지 못했습니다: 연결을 거부했으므로 연결하지 못했습니다.",
      passwordError: null,
    });
  });
});

describe("classifyStartError", () => {
  it("처음 들어올 때 passwordRequired: 폼, 오류 줄 없음", () => {
    expect(classifyStartError({ code: "passwordRequired", message: "비밀번호가 필요합니다" }, false)).toEqual({
      status: "password",
      message: "비밀번호가 필요합니다",
      passwordError: null,
    });
  });
  it("폼에서 보낸 뒤 passwordRequired·wrongPassword: 폼에 머물고 오류 줄", () => {
    expect(classifyStartError({ code: "passwordRequired", message: "m" }, true).passwordError).toBe("m");
    const wrong = classifyStartError({ code: "wrongPassword", message: "비밀번호가 맞지 않습니다" }, true);
    expect(wrong.status).toBe("password");
    expect(wrong.passwordError).toBe("비밀번호가 맞지 않습니다");
  });
  it("기억한 비밀번호가 틀려도(처음 들어올 때) 폼 + 오류 줄", () => {
    expect(classifyStartError({ code: "wrongPassword", message: "x" }, false).status).toBe("password");
  });
  it("notFound는 끝남", () => {
    expect(classifyStartError({ code: "notFound", message: "없음" }, false)).toEqual({
      status: "ended",
      message: "방송이 끝났습니다.",
      passwordError: null,
    });
    expect(classifyStartError({ code: "notFound", message: "없음" }, true).status).toBe("ended");
  });
  it("ownBroadcast는 내 방송 안내(다시 시도 없음, D-065)", () => {
    expect(classifyStartError({ code: "ownBroadcast", message: "내 방송" }, false)).toEqual({
      status: "own",
      message: "내 방송",
      passwordError: null,
    });
    expect(classifyStartError({ code: "ownBroadcast", message: "내 방송" }, true).status).toBe("own");
  });
  it("그 밖의 오류는 폼에서 보냈어도 오류 상태(연결하는 중에 머물지 않는다)", () => {
    expect(classifyStartError({ code: null, message: "서버 응답이 올바르지 않습니다" }, true)).toEqual({
      status: "error",
      message: "서버 응답이 올바르지 않습니다",
      passwordError: null,
    });
    expect(classifyStartError({ code: "other", message: "x" }, false).status).toBe("error");
  });
});

describe("상태 전이", () => {
  it("연결 상태 → 화면 상태", () => {
    expect(statusFromConnectionState("connected")).toEqual({ status: "connected" });
    expect(statusFromConnectionState("failed")).toEqual({ status: "error", message: MEDIA_FAILED_MESSAGE });
    expect(statusFromConnectionState("connecting")).toBeNull();
    expect(statusFromConnectionState("disconnected")).toBeNull();
    expect(statusFromConnectionState("closed")).toBeNull();
  });
  it("끝남·오류에서만 다시 접속", () => {
    expect(shouldResume("ended")).toBe(true);
    expect(shouldResume("error")).toBe(true);
    expect(shouldResume("connected")).toBe(false);
    expect(shouldResume("connecting")).toBe(false);
    expect(shouldResume("password")).toBe(false);
    expect(shouldResume("own")).toBe(false);
  });
  it("LIVE 배지·오른쪽 위 시청자 수", () => {
    expect(showLive("connected", true)).toBe(true);
    expect(showLive("connecting", true)).toBe(true);
    expect(showLive("password", true)).toBe(true);
    expect(showLive("error", true)).toBe(true);
    expect(showLive("ended", true)).toBe(false);
    expect(showLive("connected", false)).toBe(false);
    expect(showInfoBar("connected", true)).toBe(true);
    expect(showInfoBar("connecting", true)).toBe(true);
    expect(showInfoBar("error", true)).toBe(true);
    expect(showInfoBar("ended", true)).toBe(false);
    expect(showInfoBar("password", true)).toBe(false);
    expect(showInfoBar("connected", false)).toBe(false);
  });
  it("키 입력 가로채기 예외", () => {
    expect(isTypingTarget("INPUT", "password")).toBe(true);
    expect(isTypingTarget("input", "text")).toBe(true);
    expect(isTypingTarget("input", null)).toBe(true);
    expect(isTypingTarget("input", "range")).toBe(false);
    expect(isTypingTarget("input", "checkbox")).toBe(false);
    expect(isTypingTarget("textarea", null)).toBe(true);
    expect(isTypingTarget("button", null)).toBe(false);
    expect(isTypingTarget("div", null)).toBe(false);
  });
});

describe("toggleMute", () => {
  it("켜진 상태에서 누르면 끈다", () => {
    expect(toggleMute({ muted: false, needsUnmute: false })).toEqual({
      muted: true,
      needsUnmute: false,
      unmute: false,
    });
  });
  it("꺼진 상태에서 누르면 켠다", () => {
    expect(toggleMute({ muted: true, needsUnmute: false })).toEqual({
      muted: false,
      needsUnmute: false,
      unmute: true,
    });
  });
  it("소리 막힘(needsUnmute)에서는 한 번 눌러 바로 켠다(0.1.0은 두 번 눌러야 했다)", () => {
    expect(toggleMute({ muted: false, needsUnmute: true })).toEqual({
      muted: false,
      needsUnmute: false,
      unmute: true,
    });
    expect(toggleMute({ muted: true, needsUnmute: true }).unmute).toBe(true);
  });
  it("두 번 누르면 원래대로", () => {
    let s = { muted: false, needsUnmute: false };
    const a = toggleMute(s);
    s = { muted: a.muted, needsUnmute: a.needsUnmute };
    const b = toggleMute(s);
    expect(b.muted).toBe(false);
    expect(b.unmute).toBe(true);
  });
  it("영상 요소에 적용할 음소거와 단추 모양", () => {
    expect(effectiveMuted({ muted: false, needsUnmute: false })).toBe(false);
    expect(effectiveMuted({ muted: true, needsUnmute: false })).toBe(true);
    expect(effectiveMuted({ muted: false, needsUnmute: true })).toBe(true);
    expect(volumeIconName({ muted: false, needsUnmute: false })).toBe("volume");
    expect(volumeIconName({ muted: false, needsUnmute: true })).toBe("volume-x");
    expect(muteButtonLabel({ muted: false, needsUnmute: false })).toBe("음소거 (M)");
    expect(muteButtonLabel({ muted: true, needsUnmute: false })).toBe("소리 켜기 (M)");
  });
});

describe("checkFrozen", () => {
  const connected = { connected: true, videoKbps: 5000 };
  it("멈춤 기준은 1초", () => {
    expect(FREEZE_AFTER_MS).toBe(1000);
    expect(KEYFRAME_RETRY_MS).toBe(1000);
  });
  it("프레임이 늘면 멈춤 아님", () => {
    const r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 1000, ...connected });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastFramesDecoded).toBe(10);
    expect(r.state.lastProgressAt).toBe(1000);
  });
  it("1초 주기 통계: 마지막 프레임 뒤 다음 주기에 첫 요청, 그 뒤 1초마다 재요청, 다시 흐르면 해제", () => {
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 10_000, ...connected });
    // 11000에 프레임이 안 늘었다: 마지막 진행(10000)에서 1초 → 바로 요청
    r = checkFrozen(r.state, { framesDecoded: 10, now: 11_000, ...connected });
    expect(r.requestKeyframe).toBe(true);
    expect(r.state.lastKeyframeRequest).toBe(11_000);
    // 같은 주기 안(0.5초 뒤)에는 재요청 없음
    r = checkFrozen(r.state, { framesDecoded: 10, now: 11_500, ...connected });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 12_000, ...connected });
    expect(r.requestKeyframe).toBe(true);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 13_000, ...connected });
    expect(r.requestKeyframe).toBe(true);
    // 다시 흐르면 멈춤 해제(진행 시각 갱신), 요청 없음
    r = checkFrozen(r.state, { framesDecoded: 11, now: 14_000, ...connected });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastProgressAt).toBe(14_000);
    r = checkFrozen(r.state, { framesDecoded: 11, now: 14_500, ...connected });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 11, now: 15_000, ...connected });
    expect(r.requestKeyframe).toBe(true);
  });
  it("마지막 진행에서 잰다: 멈춘 지 오래된 뒤 처음 본 통계에서도 늦지 않다", () => {
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 1000, ...connected });
    // 통계가 1초 주기인데 프레임이 1000 이후 안 늘었다. 2000에서 요청(처음 본 시각이 아니라 마지막 진행 기준)
    r = checkFrozen(r.state, { framesDecoded: 10, now: 1900, ...connected });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 2000, ...connected });
    expect(r.requestKeyframe).toBe(true);
  });
  it("0.5초 주기에서도 같은 동작", () => {
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 5000, ...connected });
    const asked: number[] = [];
    for (let now = 5500; now <= 8000; now += 500) {
      r = checkFrozen(r.state, { framesDecoded: 10, now, ...connected });
      if (r.requestKeyframe) asked.push(now);
    }
    expect(asked).toEqual([6000, 7000, 8000]);
  });
  it("진행을 본 적 없으면 연결·수신 중으로 처음 본 시각부터 1초", () => {
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 0, now: 3000, ...connected });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastProgressAt).toBe(3000);
    r = checkFrozen(r.state, { framesDecoded: 0, now: 3999, ...connected });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 0, now: 4000, ...connected });
    expect(r.requestKeyframe).toBe(true);
  });
  it("연결 상태(connected)가 아니거나 영상 패킷이 안 오면 요청하지 않는다", () => {
    expect(checkFrozen(FREEZE_INITIAL, { framesDecoded: 0, now: 10_000, connected: false, videoKbps: 5000 })).toEqual({
      state: FREEZE_INITIAL,
      requestKeyframe: false,
    });
    const idle = checkFrozen(FREEZE_INITIAL, { framesDecoded: 0, now: 10_000, connected: true, videoKbps: 0 });
    expect(idle.requestKeyframe).toBe(false);
    expect(idle.state.lastProgressAt).toBeNull();
    // 한참 진행이 없었어도 재생 중이 아니면 요청하지 않고 기준을 지운다(다시 재생되면 그때부터 잰다)
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 1000, ...connected });
    r = checkFrozen(r.state, { framesDecoded: 10, now: 20_000, connected: false, videoKbps: 5000 });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastProgressAt).toBeNull();
    r = checkFrozen(r.state, { framesDecoded: 10, now: 20_500, ...connected });
    expect(r.requestKeyframe).toBe(false);
  });
});

describe("compactStatRows · detailedStatSections · sparklineGeometry", () => {
  const by = (rows: { label: string; value: string }[]) => Object.fromEntries(rows.map((r) => [r.label, r.value]));
  const flat = (sections: { rows: { label: string; value: string }[] }[]) => sections.flatMap((s) => s.rows);

  it("요약 모드: 5줄 + 비디오 비트레이트 스파크라인", () => {
    const tracker = new WatchStatsTracker();
    tracker.push({ now: 0, stats: stats({ videoKbps: 6500, videoPacketsLost: 0, videoPacketsReceived: 1000 }), bufferMs: 150 });
    const snapshot = tracker.push({
      now: 2000,
      stats: stats({ videoKbps: 6842, videoPacketsLost: 0, videoPacketsReceived: 2000 }),
      bufferMs: 150,
    });
    const rows = compactStatRows(stats(), { dev: false, streamIds: [], bufferMs: 150, snapshot });
    expect(rows.map((r) => r.label)).toEqual(["해상도", "비트레이트", "지연", "버퍼", "최근 10초"]);
    expect(by(rows)).toEqual({
      해상도: "1920×1080 · 60fps",
      비트레이트: "6.84 Mbps",
      지연: "84 ms · RTT 3 ms",
      버퍼: "38/42 · 목표 150 ms",
      "최근 10초": "손실 0.0% · 멈춤 0",
    });
    expect(rows.find((r) => r.key === "bitrate")?.spark).toEqual([6.5, 6.842]);
    expect(rows.find((r) => r.key === "recentQuality")?.tone).toBe("ok");
  });

  it("상세 모드: 배포 빌드 섹션(영상·네트워크·소리)과 값", () => {
    const sections = detailedStatSections(stats({ decoder: "ExternalDecoder, HW" }), {
      dev: false,
      streamIds: [],
      bufferMs: 150,
    });
    expect(sections.map((s) => s.title)).toEqual(["영상", "네트워크 · 지연", "소리"]);
    expect(by(sections[0].rows)).toEqual({
      해상도: "1920×1080 · 60fps",
      코덱: "H264 (ExternalDecoder, HW)",
      비트레이트: "6.84 Mbps",
      프레임: "버림 234 · 멈춤 42회 31.2초",
      디코딩: "3.1 ms/프레임 · 키 15",
    });
    expect(by(sections[1].rows)).toEqual({
      RTT: "3 ms",
      "캡처→재생": "84 ms",
      "A/V 차이": "+12 ms",
      "지터 버퍼": "38 / 42 ms",
      "버퍼 목표": "150 ms",
      손실: "영상 4054 · 소리 12",
      복구: "NACK 812 · 재전송 640 · PLI 9",
    });
    expect(by(sections[2].rows)).toEqual({
      비트레이트: "128 kbps",
      "소리 끊김": "은닉 37회 · 버림 4",
    });
  });

  it("audioRed: 소리 비트레이트에 · RED, 소리 끊김에서 버림을 뺀다", () => {
    const on = by(detailedStatSections(stats(), { dev: false, streamIds: [], audioRed: true })[2].rows);
    expect(on["비트레이트"]).toBe("128 kbps · RED");
    expect(on["소리 끊김"]).toBe("은닉 37회");
    const compactOn = by(compactStatRows(stats(), { dev: false, streamIds: [], audioRed: true }));
    expect(compactOn["비트레이트"]).toBe("6.84 Mbps · RED");
  });

  it("값이 없으면 –", () => {
    const sections = detailedStatSections(
      stats({
        fps: null,
        codec: null,
        videoKbps: null,
        audioKbps: null,
        latencyMs: null,
        avOffsetMs: null,
        rttMs: null,
        videoPacketsLost: null,
        audioPacketsLost: null,
        nackCount: null,
        retransmittedPacketsReceived: null,
        pliCount: null,
        totalFreezesDurationMs: null,
        keyFramesDecoded: null,
        decodeMsPerFrame: null,
        concealmentEvents: null,
        audioPacketsDiscarded: null,
      }),
      { dev: false, streamIds: [] },
    );
    const v = by(sections[0].rows);
    const net = by(sections[1].rows);
    const aud = by(sections[2].rows);
    expect(v["해상도"]).toBe("1920×1080 · –fps");
    expect(v["코덱"]).toBe("–");
    expect(v["비트레이트"]).toBe("– Mbps");
    expect(v["디코딩"]).toBe("– ms/프레임 · 키 –");
    expect(net["캡처→재생"]).toBe("– ms");
    expect(net["A/V 차이"]).toBe("– ms");
    expect(net["RTT"]).toBe("– ms");
    expect(net["손실"]).toBe("영상 – · 소리 –");
    expect(net["복구"]).toBe("NACK – · 재전송 – · PLI –");
    expect(aud["비트레이트"]).toBe("– kbps");
    expect(aud["소리 끊김"]).toBe("은닉 –회 · 버림 –");
  });

  it("캡처→재생 시계 차이와 버퍼 확장은 warn 톤", () => {
    const net = detailedStatSections(stats({ latencyMs: 16433 }), {
      dev: false,
      streamIds: [],
      bufferMs: 220,
    })[1].rows;
    expect(net.find((r) => r.key === "latency")).toMatchObject({
      value: "16433 ms · 시계 차이",
      tone: "warn",
    });
    expect(net.find((r) => r.key === "bufferTarget")).toMatchObject({
      value: "220 ms · 확장 중",
      tone: "warn",
    });
  });

  it("개발 빌드에서만 개발용 섹션이 붙는다", () => {
    const sections = detailedStatSections(
      stats({ videoTargetMs: 20.4, audioTargetMs: null, pauseCount: 2, totalPausesDurationMs: 1500 }),
      { dev: true, streamIds: ["video:a", "audio:a"] },
    );
    expect(sections.map((s) => s.id)).toEqual(["video", "network", "audio", "dev"]);
    expect(by(sections[3].rows)).toEqual({
      "연결 경로": "UDP · host→host",
      "망 지터": "영상 12.5 / 소리 4.0 ms",
      조립: "2.0 ms/프레임",
      일시정지: "2회 1.5초",
      "소리 속도": "늘림 100 · 줄임 200",
      "수신 프레임": "3000",
      "오디오 코덱": "opus",
      "목표 지연": "20 / – ms",
      스트림: "video:a audio:a",
    });
    expect(flat(sections).length).toBeGreaterThan(15);
  });

  it("sparklineGeometry: 유효 점 2개 미만이면 null, 2개 이상이면 SVG 좌표 반환", () => {
    expect(sparklineGeometry([], 56, 16)).toBeNull();
    expect(sparklineGeometry([null, 5], 56, 16)).toBeNull();
    const g = sparklineGeometry([5, 6, 5.5, 7], 56, 16);
    expect(g).not.toBeNull();
    expect(g!.line.split(" ")).toHaveLength(4);
    expect(g!.area).toContain(g!.line);
  });
});

describe("통계 고리 간격(D-106)", () => {
  it("처음 15초는 0.5초, 그 뒤 1초", () => {
    expect(STATS_FAST_PHASE_MS).toBe(15_000);
    expect(statsIntervalMs(0)).toBe(500);
    expect(statsIntervalMs(14_999)).toBe(500);
    expect(statsIntervalMs(15_000)).toBe(1000);
    expect(statsIntervalMs(120_000)).toBe(1000);
  });
});

describe("checkStreamChange(D-106)", () => {
  const at = (width: number, height: number, framesDecoded: number) => ({ width, height, framesDecoded });
  it("해상도를 처음 보면 기준만 잡는다(바뀜 아님). 해상도를 모르는 통계는 판정하지 않는다", () => {
    let r = checkStreamChange(STREAM_SIG_INITIAL, at(0, 0, 0));
    expect(r).toEqual({ changed: false, state: { size: null, framesDecoded: 0 } });
    r = checkStreamChange(r.state, at(1920, 1080, 30));
    expect(r).toEqual({ changed: false, state: { size: "1920x1080", framesDecoded: 30 } });
    r = checkStreamChange(r.state, at(1920, 1080, 60));
    expect(r.changed).toBe(false);
  });
  it("해상도가 바뀌면 바뀜", () => {
    const base = checkStreamChange(STREAM_SIG_INITIAL, at(2560, 1440, 100)).state;
    const r = checkStreamChange(base, at(1920, 1080, 130));
    expect(r).toEqual({ changed: true, state: { size: "1920x1080", framesDecoded: 130 } });
    expect(checkStreamChange(r.state, at(1920, 1080, 160)).changed).toBe(false);
  });
  it("같은 해상도에서 framesDecoded가 줄면 바뀜(디코더를 새로 만듦)", () => {
    const base = checkStreamChange(STREAM_SIG_INITIAL, at(1920, 1080, 500)).state;
    expect(checkStreamChange(base, at(1920, 1080, 500)).changed).toBe(false);
    expect(checkStreamChange(base, at(1920, 1080, 3)).changed).toBe(true);
  });
  it("해상도를 모르는 통계(width 0)는 해상도 기준을 두고 프레임 수만 새로 적는다", () => {
    const base = checkStreamChange(STREAM_SIG_INITIAL, at(1920, 1080, 500)).state;
    const blank = checkStreamChange(base, at(0, 0, 2));
    expect(blank).toEqual({ changed: false, state: { size: "1920x1080", framesDecoded: 2 } });
    // 줄어든 값이 이미 기준이 됐으니 다음 통계는 바뀜이 아니다
    expect(checkStreamChange(blank.state, at(1920, 1080, 10)).changed).toBe(false);
  });
});

describe("devLogLine", () => {
  it("앱 로그 한 줄", () => {
    const line = devLogLine(stats({ videoTargetMs: 20, audioTargetMs: 30 }), false);
    expect(line).toBe(
      "watch 1920x1080 fps=60 kbps=6842+128 latency=84 av=12 jb=38/42 target=20/30 lost=4066 dropped=234 freeze=42 codec=H264 hidden=false lost_va=4054/12 nack=812 rtx=640 pli=9 key=15 dec=3.1",
    );
    expect(devLogLine(stats({ fps: null, latencyMs: null }), true)).toContain("fps=- kbps=6842+128 latency=-");
  });
  it("시청 버퍼를 주면 hidden 뒤에 buf=", () => {
    expect(devLogLine(stats(), false, 115)).toContain(" hidden=false buf=115 lost_va=4054/12 ");
    expect(devLogLine(stats(), false, 115).includes(" red=")).toBe(false);
    expect(devLogLine(stats(), false, 115, true).endsWith(" red=1")).toBe(true);
    expect(devLogLine(stats(), false, undefined, false).endsWith(" red=0")).toBe(true);
  });
  it("값이 없는 진단 필드는 -", () => {
    const line = devLogLine(
      stats({ videoPacketsLost: null, nackCount: null, retransmittedPacketsReceived: null, pliCount: null, keyFramesDecoded: null, decodeMsPerFrame: null }),
      false,
    );
    expect(line).toMatch(/ lost_va=-\/12 nack=- rtx=- pli=- key=- dec=-$/);
  });
});
