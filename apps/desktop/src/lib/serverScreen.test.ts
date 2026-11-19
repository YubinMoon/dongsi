import { describe, expect, it } from "vitest";
import { addrLine, broadcastCount, publishHref, serverScreen, tileMeta, watchHref } from "./serverScreen";

const base = { listLoaded: true, exists: true, connected: false, closedTab: false };

describe("serverScreen", () => {
  it("연결돼 있으면 live", () => {
    expect(serverScreen({ ...base, connected: true })).toBe("live");
  });
  it("끊긴 탭이면 closed (연결 표시와 겹쳐도 closed가 먼저)", () => {
    expect(serverScreen({ ...base, closedTab: true })).toBe("closed");
    expect(serverScreen({ ...base, closedTab: true, connected: true })).toBe("closed");
  });
  it("연결도 끊긴 탭도 아니면 connecting", () => {
    expect(serverScreen(base)).toBe("connecting");
  });
  it("목록에 없으면 missing, 목록을 아직 못 읽었으면 loading", () => {
    expect(serverScreen({ ...base, exists: false })).toBe("missing");
    expect(serverScreen({ ...base, exists: false, listLoaded: false })).toBe("loading");
    expect(serverScreen({ ...base, exists: false, connected: true })).toBe("missing");
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
});
