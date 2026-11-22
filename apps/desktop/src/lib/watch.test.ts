import { describe, expect, it } from "vitest";
import {
  backHref,
  cardFailureMessage,
  checkFrozen,
  classifyStartError,
  devLogLine,
  effectiveMuted,
  endedDescription,
  FREEZE_AFTER_MS,
  FREEZE_INITIAL,
  isPopoutHash,
  isTypingTarget,
  KEYFRAME_RETRY_MS,
  lipsyncLabel,
  MEDIA_FAILED_MESSAGE,
  muteButtonLabel,
  networkLabel,
  type NetworkLabel,
  passwordLabel,
  passwordSubtitle,
  shouldResume,
  showInfoBar,
  showLive,
  statRows,
  statusFromConnectionState,
  toggleMute,
  viewersLabel,
  volumeIconName,
} from "./watch";
import type { WatchStats } from "./watchStats";

function stats(over: Partial<WatchStats> = {}): WatchStats {
  return {
    width: 1920,
    height: 1080,
    fps: 60,
    codec: "H264",
    decoder: null,
    videoKbps: 6842,
    audioKbps: 128,
    latencyMs: 84,
    avOffsetMs: 12,
    videoJitterMs: 38,
    audioJitterMs: 42,
    rttMs: 3,
    packetsLost: 4066,
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
  it("passwordSubtitle: 4K는 4K로", () => {
    expect(passwordSubtitle("도윤", { codec: "av1", height: 2160, fps: 30 })).toBe("도윤 · 4K · 30fps · AV1");
    expect(passwordSubtitle("하린", { codec: "h264", height: 1080, fps: 60 })).toBe("하린 · 1080p · 60fps · H.264");
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
    expect(statusFromConnectionState("connected")).toEqual({ status: "playing" });
    expect(statusFromConnectionState("failed")).toEqual({ status: "error", message: MEDIA_FAILED_MESSAGE });
    expect(statusFromConnectionState("connecting")).toBeNull();
    expect(statusFromConnectionState("disconnected")).toBeNull();
    expect(statusFromConnectionState("closed")).toBeNull();
  });
  it("끝남·오류에서만 다시 접속", () => {
    expect(shouldResume("ended")).toBe(true);
    expect(shouldResume("error")).toBe(true);
    expect(shouldResume("playing")).toBe(false);
    expect(shouldResume("connecting")).toBe(false);
    expect(shouldResume("password")).toBe(false);
    expect(shouldResume("own")).toBe(false);
  });
  it("LIVE 배지·오른쪽 위 시청자 수", () => {
    expect(showLive("playing", true)).toBe(true);
    expect(showLive("connecting", true)).toBe(true);
    expect(showLive("password", true)).toBe(true);
    expect(showLive("error", true)).toBe(true);
    expect(showLive("ended", true)).toBe(false);
    expect(showLive("playing", false)).toBe(false);
    expect(showInfoBar("playing", true)).toBe(true);
    expect(showInfoBar("connecting", true)).toBe(true);
    expect(showInfoBar("error", true)).toBe(true);
    expect(showInfoBar("ended", true)).toBe(false);
    expect(showInfoBar("password", true)).toBe(false);
    expect(showInfoBar("playing", false)).toBe(false);
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
  const playing = { playing: true, videoKbps: 5000 };
  it("멈춤 기준은 1초", () => {
    expect(FREEZE_AFTER_MS).toBe(1000);
    expect(KEYFRAME_RETRY_MS).toBe(1000);
  });
  it("프레임이 늘면 멈춤 아님", () => {
    const r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 1000, ...playing });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastFramesDecoded).toBe(10);
    expect(r.state.lastProgressAt).toBe(1000);
  });
  it("1초 주기 통계: 마지막 프레임 뒤 다음 주기에 첫 요청, 그 뒤 1초마다 재요청, 다시 흐르면 해제", () => {
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 10_000, ...playing });
    // 11000에 프레임이 안 늘었다: 마지막 진행(10000)에서 1초 → 바로 요청
    r = checkFrozen(r.state, { framesDecoded: 10, now: 11_000, ...playing });
    expect(r.requestKeyframe).toBe(true);
    expect(r.state.lastKeyframeRequest).toBe(11_000);
    // 같은 주기 안(0.5초 뒤)에는 재요청 없음
    r = checkFrozen(r.state, { framesDecoded: 10, now: 11_500, ...playing });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 12_000, ...playing });
    expect(r.requestKeyframe).toBe(true);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 13_000, ...playing });
    expect(r.requestKeyframe).toBe(true);
    // 다시 흐르면 멈춤 해제(진행 시각 갱신), 요청 없음
    r = checkFrozen(r.state, { framesDecoded: 11, now: 14_000, ...playing });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastProgressAt).toBe(14_000);
    r = checkFrozen(r.state, { framesDecoded: 11, now: 14_500, ...playing });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 11, now: 15_000, ...playing });
    expect(r.requestKeyframe).toBe(true);
  });
  it("마지막 진행에서 잰다: 멈춘 지 오래된 뒤 처음 본 통계에서도 늦지 않다", () => {
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 1000, ...playing });
    // 통계가 1초 주기인데 프레임이 1000 이후 안 늘었다. 2000에서 요청(처음 본 시각이 아니라 마지막 진행 기준)
    r = checkFrozen(r.state, { framesDecoded: 10, now: 1900, ...playing });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 2000, ...playing });
    expect(r.requestKeyframe).toBe(true);
  });
  it("0.5초 주기에서도 같은 동작", () => {
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 5000, ...playing });
    const asked: number[] = [];
    for (let now = 5500; now <= 8000; now += 500) {
      r = checkFrozen(r.state, { framesDecoded: 10, now, ...playing });
      if (r.requestKeyframe) asked.push(now);
    }
    expect(asked).toEqual([6000, 7000, 8000]);
  });
  it("진행을 본 적 없으면 재생·수신 중으로 처음 본 시각부터 1초", () => {
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 0, now: 3000, ...playing });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastProgressAt).toBe(3000);
    r = checkFrozen(r.state, { framesDecoded: 0, now: 3999, ...playing });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 0, now: 4000, ...playing });
    expect(r.requestKeyframe).toBe(true);
  });
  it("재생 중이 아니거나 영상 패킷이 안 오면 요청하지 않는다", () => {
    expect(checkFrozen(FREEZE_INITIAL, { framesDecoded: 0, now: 10_000, playing: false, videoKbps: 5000 })).toEqual({
      state: FREEZE_INITIAL,
      requestKeyframe: false,
    });
    const idle = checkFrozen(FREEZE_INITIAL, { framesDecoded: 0, now: 10_000, playing: true, videoKbps: 0 });
    expect(idle.requestKeyframe).toBe(false);
    expect(idle.state.lastProgressAt).toBeNull();
    // 한참 진행이 없었어도 재생 중이 아니면 요청하지 않고 기준을 지운다(다시 재생되면 그때부터 잰다)
    let r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 1000, ...playing });
    r = checkFrozen(r.state, { framesDecoded: 10, now: 20_000, playing: false, videoKbps: 5000 });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastProgressAt).toBeNull();
    r = checkFrozen(r.state, { framesDecoded: 10, now: 20_500, ...playing });
    expect(r.requestKeyframe).toBe(false);
  });
});

describe("networkLabel", () => {
  const base: NetworkLabel = {
    kind: "wifi",
    viaTunnel: false,
    signalQuality: 78,
    rssiDbm: -55,
    rxRateMbps: 866,
    channel: 36,
    streamingMode: false,
  };
  it("유선·기타·알 수 없음", () => {
    expect(networkLabel({ ...base, kind: "wired", signalQuality: null, rxRateMbps: null })).toBe("유선");
    expect(networkLabel({ ...base, kind: "other" })).toBe("기타");
    expect(networkLabel({ ...base, kind: "unknown" })).toBe("알 수 없음");
  });
  it("Wi-Fi: 신호·속도, 없는 조각은 뺀다", () => {
    expect(networkLabel(base)).toBe("Wi-Fi · 신호 78% · 866Mbps");
    expect(networkLabel({ ...base, rxRateMbps: null })).toBe("Wi-Fi · 신호 78%");
    expect(networkLabel({ ...base, signalQuality: null })).toBe("Wi-Fi · 866Mbps");
    expect(networkLabel({ ...base, signalQuality: null, rxRateMbps: null })).toBe("Wi-Fi");
  });
  it("VPN은 앞에, 스트리밍 모드는 끝에", () => {
    expect(networkLabel({ ...base, viaTunnel: true, rxRateMbps: null })).toBe("VPN · Wi-Fi · 신호 78%");
    expect(networkLabel({ ...base, streamingMode: true })).toBe("Wi-Fi · 신호 78% · 866Mbps · 스트리밍 모드");
    expect(networkLabel({ ...base, kind: "wired", viaTunnel: true, streamingMode: true })).toBe(
      "VPN · 유선 · 스트리밍 모드",
    );
  });
});

describe("statRows", () => {
  const by = (rows: { label: string; value: string }[]) => Object.fromEntries(rows.map((r) => [r.label, r.value]));
  it("배포 빌드 줄 순서와 값(예시 통계)", () => {
    const rows = statRows(stats({ decoder: "ExternalDecoder, HW" }), { dev: false, streamIds: [] });
    expect(rows.map((r) => r.label)).toEqual([
      "해상도",
      "코덱",
      "비트레이트",
      "캡처→재생",
      "A/V 차이",
      "지터 버퍼",
      "RTT",
      "손실",
      "복구",
      "프레임",
      "디코딩",
      "소리 끊김",
    ]);
    expect(by(rows)).toEqual({
      해상도: "1920×1080 · 60fps",
      코덱: "H264 (ExternalDecoder, HW)",
      비트레이트: "6.84 Mbps + 128 kbps",
      "캡처→재생": "84 ms",
      "A/V 차이": "12 ms",
      "지터 버퍼": "38 / 42 ms",
      RTT: "3 ms",
      손실: "영상 4054 · 소리 12",
      복구: "NACK 812 · 재전송 640 · PLI 9",
      프레임: "버림 234 · 멈춤 42회 31.2초 · 키 15",
      디코딩: "3.1 ms/프레임",
      "소리 끊김": "은닉 37회 · 버림 4",
    });
  });
  it("값이 없으면 –", () => {
    const rows = statRows(
      stats({
        fps: null,
        codec: null,
        latencyMs: null,
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
    const v = by(rows);
    expect(v["해상도"]).toBe("1920×1080 · –fps");
    expect(v["코덱"]).toBe("–");
    expect(v["캡처→재생"]).toBe("– ms");
    expect(v["RTT"]).toBe("– ms");
    expect(v["손실"]).toBe("영상 – · 소리 –");
    expect(v["복구"]).toBe("NACK – · 재전송 – · PLI –");
    expect(v["프레임"]).toBe("버림 234 · 멈춤 42회 –초 · 키 –");
    expect(v["디코딩"]).toBe("– ms/프레임");
    expect(v["소리 끊김"]).toBe("은닉 –회 · 버림 –");
  });
  it("캡처→재생: 0 미만이거나 2000ms 초과면 시계 차이", () => {
    const value = (ms: number | null) =>
      by(statRows(stats({ latencyMs: ms }), { dev: false, streamIds: [] }))["캡처→재생"];
    expect(value(84)).toBe("84 ms");
    expect(value(2000)).toBe("2000 ms");
    expect(value(0)).toBe("0 ms");
    expect(value(16433)).toBe("16433 ms · 시계 차이");
    expect(value(2001)).toBe("2001 ms · 시계 차이");
    expect(value(-35)).toBe("-35 ms · 시계 차이");
    expect(value(null)).toBe("– ms");
  });
  it("네트워크 줄은 network를 줄 때만, 비트레이트 다음에", () => {
    const net: NetworkLabel = {
      kind: "wifi",
      viaTunnel: true,
      signalQuality: 78,
      rssiDbm: null,
      rxRateMbps: null,
      channel: null,
      streamingMode: false,
    };
    const rows = statRows(stats(), { dev: false, streamIds: [], network: net });
    const labels = rows.map((r) => r.label);
    expect(labels.indexOf("네트워크")).toBe(labels.indexOf("비트레이트") + 1);
    expect(by(rows)["네트워크"]).toBe("VPN · Wi-Fi · 신호 78%");
    expect(statRows(stats(), { dev: false, streamIds: [], network: null }).map((r) => r.label)).not.toContain("네트워크");
    expect(statRows(stats(), { dev: false, streamIds: [] }).map((r) => r.label)).not.toContain("네트워크");
  });
  it("개발 빌드에서만 진단 줄과 목표 지연, 스트림 줄", () => {
    const rows = statRows(
      stats({ videoTargetMs: 20.4, audioTargetMs: null, pauseCount: 2, totalPausesDurationMs: 1500 }),
      { dev: true, streamIds: ["video:a", "audio:a"] },
    );
    expect(rows.slice(-8)).toEqual([
      { label: "망 지터", value: "영상 12.5 / 소리 4.0 ms" },
      { label: "조립", value: "2.0 ms/프레임" },
      { label: "일시정지", value: "2회 1.5초" },
      { label: "소리 속도", value: "늘림 100 · 줄임 200" },
      { label: "수신 프레임", value: "3000" },
      { label: "오디오 코덱", value: "opus" },
      { label: "목표 지연", value: "20 / – ms" },
      { label: "스트림", value: "video:a audio:a" },
    ]);
  });
  it("시청 버퍼(D-072)를 주면 지터 버퍼 다음에 버퍼 목표 줄(배포 빌드에도)", () => {
    const rows = statRows(stats(), { dev: false, streamIds: [], bufferMs: 145 });
    const labels = rows.map((r) => r.label);
    expect(labels.indexOf("버퍼 목표")).toBe(labels.indexOf("지터 버퍼") + 1);
    expect(rows.find((r) => r.label === "버퍼 목표")?.value).toBe("145 ms");
    expect(rows).toHaveLength(13);
  });
});

describe("립싱크 보정 표시", () => {
  it("lipsyncLabel", () => {
    expect(lipsyncLabel({ enabled: true, adjustMs: 120 })).toBe("비디오 +120ms");
    expect(lipsyncLabel({ enabled: true, adjustMs: -30 })).toBe("오디오 +30ms");
    expect(lipsyncLabel({ enabled: true, adjustMs: 0 })).toBe("0ms");
    expect(lipsyncLabel({ enabled: false, adjustMs: 80 })).toBe("꺼짐");
  });
  it("lipsyncLabel: 불안정·회복 중이면 끝에 보류가 붙는다", () => {
    expect(lipsyncLabel({ enabled: true, adjustMs: 120, state: "stable" })).toBe("비디오 +120ms");
    expect(lipsyncLabel({ enabled: true, adjustMs: 120, state: "unstable" })).toBe("비디오 +120ms · 보류");
    expect(lipsyncLabel({ enabled: true, adjustMs: -30, state: "recovering" })).toBe("오디오 +30ms · 보류");
    expect(lipsyncLabel({ enabled: true, adjustMs: 0, state: "unstable" })).toBe("0ms · 보류");
    // 꺼져 있으면 상태와 무관하게 "꺼짐"
    expect(lipsyncLabel({ enabled: false, adjustMs: 0, state: "unstable" })).toBe("꺼짐");
  });
  it("개발 빌드에서만 목표 지연과 스트림 사이에 줄이 붙는다", () => {
    const opts = { streamIds: ["video:a"], lipsync: { enabled: true, adjustMs: 95 } };
    const dev = statRows(stats(), { dev: true, ...opts });
    expect(dev.map((r) => r.label).slice(-3)).toEqual(["목표 지연", "립싱크 보정", "스트림"]);
    expect(dev.find((r) => r.label === "립싱크 보정")?.value).toBe("비디오 +95ms");
    const prod = statRows(stats(), { dev: false, ...opts });
    expect(prod.map((r) => r.label)).not.toContain("립싱크 보정");
    expect(prod).toHaveLength(12);
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
  });
  it("값이 없는 진단 필드는 -", () => {
    const line = devLogLine(
      stats({ videoPacketsLost: null, nackCount: null, retransmittedPacketsReceived: null, pliCount: null, keyFramesDecoded: null, decodeMsPerFrame: null }),
      false,
    );
    expect(line).toMatch(/ lost_va=-\/12 nack=- rtx=- pli=- key=- dec=-$/);
  });
});
