import { describe, expect, it } from "vitest";
import {
  backHref,
  cardFailureMessage,
  checkFrozen,
  classifyStartError,
  devLogLine,
  effectiveMuted,
  endedDescription,
  FREEZE_INITIAL,
  infoLine,
  isPopoutHash,
  isTypingTarget,
  lipsyncLabel,
  MEDIA_FAILED_MESSAGE,
  muteButtonLabel,
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
    packetsLost: 0,
    framesDropped: 0,
    freezeCount: 0,
    framesDecoded: 100,
    audioPackets: 250,
    videoTargetMs: null,
    audioTargetMs: null,
    audioConcealedSamples: 0,
    ...over,
  };
}

describe("backHref·isPopoutHash", () => {
  it("돌아가기 대상은 0.1.0과 같다", () => {
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
  it("infoLine", () => {
    expect(infoLine({ codec: "h264", width: 1920, height: 1080, fps: 60 })).toBe("H.264 · 1920×1080 · 60fps");
    expect(infoLine({ codec: "av1", width: 3840, height: 2160, fps: 30 })).toBe("AV1 · 3840×2160 · 30fps");
  });
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
  });
  it("LIVE 배지·오른쪽 위 정보", () => {
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
  it("프레임이 늘면 멈춤 아님", () => {
    const r = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 1000, ...playing });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.lastFramesDecoded).toBe(10);
    expect(r.state.frozenSince).toBeNull();
  });
  it("2초 넘게 안 늘면 키프레임을 요청하고, 다시 2초 뒤에야 또 요청", () => {
    let s = checkFrozen(FREEZE_INITIAL, { framesDecoded: 10, now: 1000, ...playing }).state;
    let r = checkFrozen(s, { framesDecoded: 10, now: 2000, ...playing });
    expect(r.requestKeyframe).toBe(false);
    expect(r.state.frozenSince).toBe(2000);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 3000, ...playing });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 4100, ...playing });
    expect(r.requestKeyframe).toBe(true);
    expect(r.state.lastKeyframeRequest).toBe(4100);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 5000, ...playing });
    expect(r.requestKeyframe).toBe(false);
    r = checkFrozen(r.state, { framesDecoded: 10, now: 6200, ...playing });
    expect(r.requestKeyframe).toBe(true);
    // 다시 흐르면 멈춤 기록을 지운다.
    r = checkFrozen(r.state, { framesDecoded: 11, now: 7000, ...playing });
    expect(r.state.frozenSince).toBeNull();
  });
  it("재생 중이 아니거나 영상 패킷이 안 오면 세지 않는다", () => {
    expect(checkFrozen(FREEZE_INITIAL, { framesDecoded: 0, now: 10_000, playing: false, videoKbps: 5000 })).toEqual({
      state: FREEZE_INITIAL,
      requestKeyframe: false,
    });
    expect(checkFrozen(FREEZE_INITIAL, { framesDecoded: 0, now: 10_000, playing: true, videoKbps: 0 }).state.frozenSince).toBeNull();
  });
});

describe("statRows", () => {
  it("0.1.0과 같은 값, 시안과 같은 순서", () => {
    const rows = statRows(stats({ decoder: "ExternalDecoder, HW" }), { dev: false, streamIds: [] });
    expect(rows.map((r) => r.label)).toEqual([
      "해상도",
      "코덱",
      "비트레이트",
      "지연",
      "A/V 차이",
      "지터 버퍼",
      "RTT",
      "손실",
    ]);
    const by = Object.fromEntries(rows.map((r) => [r.label, r.value]));
    expect(by["해상도"]).toBe("1920×1080 · 60fps");
    expect(by["코덱"]).toBe("H264 (ExternalDecoder, HW)");
    expect(by["비트레이트"]).toBe("6842 + 128 kbps");
    expect(by["지연"]).toBe("84 ms");
    expect(by["A/V 차이"]).toBe("12 ms");
    expect(by["지터 버퍼"]).toBe("38 / 42 ms");
    expect(by["RTT"]).toBe("3 ms");
    expect(by["손실"]).toBe("0 · 버린 프레임 0 · 멈춤 0");
  });
  it("값이 없으면 –", () => {
    const rows = statRows(stats({ fps: null, codec: null, latencyMs: null, rttMs: null }), { dev: false, streamIds: [] });
    const by = Object.fromEntries(rows.map((r) => [r.label, r.value]));
    expect(by["해상도"]).toBe("1920×1080 · –fps");
    expect(by["코덱"]).toBe("–");
    expect(by["지연"]).toBe("– ms");
    expect(by["RTT"]).toBe("– ms");
  });
  it("개발 빌드에서만 목표 지연과 스트림 줄", () => {
    const rows = statRows(stats({ videoTargetMs: 20.4, audioTargetMs: null }), {
      dev: true,
      streamIds: ["video:a", "audio:a"],
    });
    expect(rows.slice(-2)).toEqual([
      { label: "목표 지연", value: "20 / – ms" },
      { label: "스트림", value: "video:a audio:a" },
    ]);
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
    expect(prod).toHaveLength(8);
  });
});

describe("devLogLine", () => {
  it("앱 로그 한 줄", () => {
    const line = devLogLine(stats({ videoTargetMs: 20, audioTargetMs: 30 }), false);
    expect(line).toBe(
      "watch 1920x1080 fps=60 kbps=6842+128 latency=84 av=12 jb=38/42 target=20/30 lost=0 dropped=0 freeze=0 codec=H264 hidden=false",
    );
    expect(devLogLine(stats({ fps: null, latencyMs: null }), true)).toContain("fps=- kbps=6842+128 latency=-");
  });
});
