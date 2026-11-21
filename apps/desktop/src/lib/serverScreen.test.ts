import { describe, expect, it } from "vitest";
import {
  addrLine,
  broadcastCount,
  downView,
  fingerprintPrefix,
  publishHref,
  serverScreen,
  tileHref,
  tileMeta,
  watchHref,
} from "./serverScreen";

const base = { listLoaded: true, exists: true, status: "idle" as const };

describe("serverScreen", () => {
  it("연결돼 있으면 live", () => {
    expect(serverScreen({ ...base, status: "connected" })).toBe("live");
  });
  it("연결하는 중이거나 시도 전이면 connecting", () => {
    expect(serverScreen({ ...base, status: "connecting" })).toBe("connecting");
    expect(serverScreen(base)).toBe("connecting");
  });
  it("그 밖의 연결 안 된 상태는 down", () => {
    for (const status of ["closed", "offline", "failed", "fingerprint", "paused"] as const) {
      expect(serverScreen({ ...base, status })).toBe("down");
    }
  });
  it("목록에 없으면 missing, 목록을 아직 못 읽었으면 loading", () => {
    expect(serverScreen({ ...base, exists: false })).toBe("missing");
    expect(serverScreen({ ...base, exists: false, listLoaded: false })).toBe("loading");
    expect(serverScreen({ ...base, exists: false, status: "connected" })).toBe("missing");
  });
});

describe("downView", () => {
  it("끊김: 한 줄 + 이유(D-055)", () => {
    expect(downView({ kind: "closed", reason: "서버가 닫혔습니다.", retrying: false })).toEqual({
      tone: "neutral",
      icon: "alert",
      title: "서버와 연결이 끊어졌습니다.",
      detail: "서버가 닫혔습니다.",
    });
    expect(downView({ kind: "closed", reason: null, retrying: false })?.detail).toBeNull();
  });
  it("자동 연결 실패는 중립, 사용자가 눌러 실패는 빨강. 설명은 전체 문장", () => {
    expect(downView({ kind: "offline", message: "짧게", title: "전체 문장", retrying: false })).toMatchObject({
      tone: "neutral",
      title: "서버에 연결하지 못했습니다.",
      detail: "전체 문장",
    });
    expect(downView({ kind: "failed", message: "짧게", title: "전체 문장", retrying: false })?.tone).toBe("danger");
  });
  it("사용자가 끊음", () => {
    expect(downView({ kind: "paused" })).toMatchObject({ title: "연결을 끊었습니다." });
  });
  it("지문 경고·연결됨·연결하는 중은 따로 그린다", () => {
    expect(downView({ kind: "fingerprint", message: "x", compare: null })).toBeNull();
    expect(downView({ kind: "connected" })).toBeNull();
    expect(downView({ kind: "connecting" })).toBeNull();
  });
});

describe("fingerprintPrefix", () => {
  it("앞 네 묶음만 남긴다", () => {
    expect(fingerprintPrefix("3A:9F:12:C4:7B:E0:55:D1")).toBe("3A:9F:12:C4");
  });
  it("묶음이 적으면 그대로", () => {
    expect(fingerprintPrefix("3A:9F")).toBe("3A:9F");
    expect(fingerprintPrefix("")).toBe("");
  });
  it("묶음 수를 정할 수 있다", () => {
    expect(fingerprintPrefix("AA:BB:CC", 2)).toBe("AA:BB");
  });
});

describe("표시 문자열", () => {
  it("주소 줄", () => {
    expect(addrLine(["dongsi.example.net", "100.101.102.103"])).toBe("dongsi.example.net, 100.101.102.103");
    expect(addrLine([])).toBe("");
  });
  it("타일 아래 줄", () => {
    expect(tileMeta({ height: 1080, fps: 60, codec: "h264" })).toBe("1080p · 60fps · H.264");
    expect(tileMeta({ height: 2160, fps: 30, codec: "av1" })).toBe("4K · 30fps · AV1");
  });
  it("방송 수·주소", () => {
    expect(broadcastCount(3)).toBe("3명");
    expect(broadcastCount(0)).toBe("0명");
    expect(watchHref("s1", "b1")).toBe("#/watch/s1/b1");
    expect(publishHref("s1")).toBe("#/broadcast?server=s1");
  });
  it("타일 링크: 내 방송은 시청 화면이 아니라 방송 화면(D-065)", () => {
    expect(tileHref("s1", { broadcasterId: "b1", mine: false })).toBe("#/watch/s1/b1");
    expect(tileHref("s1", { broadcasterId: "b1", mine: true })).toBe("#/broadcast");
  });
});
