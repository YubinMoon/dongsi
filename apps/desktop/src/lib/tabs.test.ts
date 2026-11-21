import { describe, expect, it } from "vitest";
import {
  addConnected,
  computeTabs,
  mergeBackend,
  removeKey,
  serverMenu,
  serverStatus,
  tabStatusText,
  tabTooltip,
  type ConnSnapshot,
} from "./tabs";

const snap = (o: Partial<ConnSnapshot> = {}): ConnSnapshot => ({
  connected: [],
  card: {},
  trying: {},
  paused: {},
  closedReason: {},
  ...o,
});
const fp = (short: string) => ({ short, full: short + ":FULL" });

describe("serverStatus", () => {
  it("연결돼 있으면 다른 기록이 있어도 connected", () => {
    expect(serverStatus("a", snap({ connected: ["a"], card: { a: { kind: "offline", message: "x" } } }))).toEqual({
      kind: "connected",
    });
  });

  it("아무 기록이 없으면 idle, 사용자가 끊었으면 paused", () => {
    expect(serverStatus("a", snap())).toEqual({ kind: "idle" });
    expect(serverStatus("a", snap({ paused: { a: true } }))).toEqual({ kind: "paused" });
  });

  it("시도 중: 사용자가 누른 시도, 또는 실패한 적 없는 자동 연결이면 connecting", () => {
    expect(serverStatus("a", snap({ card: { a: { kind: "connecting" } }, trying: { a: true } }))).toEqual({
      kind: "connecting",
    });
    expect(serverStatus("a", snap({ trying: { a: true } }))).toEqual({ kind: "connecting" });
    // 사용자가 끊었던 서버를 다시 시도하는 중이어도 connecting
    expect(serverStatus("a", snap({ trying: { a: true }, paused: { a: true } }))).toEqual({ kind: "connecting" });
  });

  it("자동 연결 실패(offline)는 짧은 이유와 전체 문장, 다시 시도하는 동안에도 offline(깜빡이지 않게)", () => {
    const message =
      "100.88.12.7:7900에 접속하지 못했습니다: 대상 컴퓨터에서 연결을 거부했으므로 연결하지 못했습니다. (os error 10061)";
    expect(serverStatus("a", snap({ card: { a: { kind: "offline", message } } }))).toEqual({
      kind: "offline",
      message: "접속하지 못했습니다(연결 거부)",
      title: "100.88.12.7:7900에 접속하지 못했습니다: 대상 컴퓨터에서 연결을 거부했으므로 연결하지 못했습니다.",
      retrying: false,
    });
    expect(serverStatus("a", snap({ card: { a: { kind: "offline", message } }, trying: { a: true } }))).toMatchObject({
      kind: "offline",
      retrying: true,
    });
  });

  it("사용자가 눌러 실패(failed)", () => {
    expect(serverStatus("a", snap({ card: { a: { kind: "failed", message: "접속하지 못했습니다(시간 초과)" } } }))).toEqual({
      kind: "failed",
      message: "접속하지 못했습니다(시간 초과)",
      title: "접속하지 못했습니다(시간 초과)",
      retrying: false,
    });
  });

  it("지문 불일치: 두 지문이 다 있으면 비교, 하나라도 없으면 message만", () => {
    const both = serverStatus(
      "a",
      snap({ card: { a: { kind: "fingerprint", message: "다릅니다", stored: fp("A"), actual: fp("B") } } }),
    );
    expect(both).toEqual({ kind: "fingerprint", message: "다릅니다", compare: { stored: fp("A"), actual: fp("B") } });
    const noActual = serverStatus(
      "a",
      snap({ card: { a: { kind: "fingerprint", message: "다릅니다", stored: fp("A"), actual: null } } }),
    );
    expect(noActual).toEqual({ kind: "fingerprint", message: "다릅니다", compare: null });
  });

  it("끊긴 서버: 이유가 기본 문구면 null, 뒤의 자동 재시도 실패(offline)보다 먼저", () => {
    expect(serverStatus("a", snap({ closedReason: { a: "서버가 닫혔습니다." } }))).toEqual({
      kind: "closed",
      reason: "서버가 닫혔습니다.",
      retrying: false,
    });
    expect(serverStatus("a", snap({ closedReason: { a: "서버와 연결이 끊어졌습니다" } }))).toMatchObject({
      kind: "closed",
      reason: null,
    });
    expect(
      serverStatus("a", snap({ closedReason: { a: "x" }, card: { a: { kind: "offline", message: "거부" } }, trying: { a: true } })),
    ).toMatchObject({ kind: "closed", retrying: true });
    // 사용자가 눌러 실패하면 그 실패를 보인다
    expect(serverStatus("a", snap({ closedReason: { a: "x" }, card: { a: { kind: "failed", message: "거부" } } })).kind).toBe(
      "failed",
    );
  });
});

describe("computeTabs", () => {
  const servers = [
    { id: "a", name: "우리집" },
    { id: "b", name: "민수 PC" },
    { id: "c", name: "회사" },
  ];

  it("저장한 서버를 저장 순서대로 모두, 연결 안 된 서버도", () => {
    const tabs = computeTabs(servers, snap({ connected: ["c"] }), null);
    expect(tabs.map((t) => t.id)).toEqual(["a", "b", "c"]);
    expect(tabs.map((t) => t.status.kind)).toEqual(["idle", "idle", "connected"]);
  });

  it("저장하지 않은 서버(감지한 방송을 보느라 연결한 서버)는 탭에 없다", () => {
    const tabs = computeTabs(servers, snap({ connected: ["local-xyz", "a"] }), null);
    expect(tabs.map((t) => t.id)).toEqual(["a", "b", "c"]);
  });

  it("active, 툴팁, 화면 읽기용 상태", () => {
    const tabs = computeTabs(
      servers,
      snap({ connected: ["a"], card: { b: { kind: "offline", message: "서버가 응답하지 않습니다" } } }),
      "b",
    );
    expect(tabs[0]).toMatchObject({ id: "a", name: "우리집", active: false, tooltip: null, statusText: null });
    expect(tabs[1]).toMatchObject({
      id: "b",
      active: true,
      tooltip: "서버가 응답하지 않습니다",
      statusText: "연결 안 됨",
    });
  });

  it("비어 있으면 빈 배열", () => {
    expect(computeTabs([], snap({ connected: ["a"] }), null)).toEqual([]);
  });
});

describe("tabTooltip·tabStatusText", () => {
  it("상태마다 문구", () => {
    expect(tabTooltip({ kind: "connecting" })).toBe("연결하는 중…");
    expect(tabTooltip({ kind: "closed", reason: null, retrying: false })).toBe("서버와 연결이 끊어졌습니다.");
    expect(tabTooltip({ kind: "closed", reason: "서버가 닫혔습니다.", retrying: false })).toBe(
      "서버와 연결이 끊어졌습니다: 서버가 닫혔습니다.",
    );
    expect(tabTooltip({ kind: "fingerprint", message: "x", compare: null })).toContain("지문");
    expect(tabTooltip({ kind: "idle" })).toBeNull();
    expect(tabStatusText({ kind: "connected" })).toBeNull();
    expect(tabStatusText({ kind: "paused" })).toBe("연결 끊음");
  });
});

describe("serverMenu", () => {
  const actions = (kind: Parameters<typeof serverMenu>[0]) => serverMenu(kind).map((e) => e.action);
  it("연결돼 있으면 연결 끊기, 아니면 다시 연결, 연결하는 중이면 둘 다 없음", () => {
    expect(actions({ kind: "connected" })).toEqual(["disconnect", "rename", "remove"]);
    expect(actions({ kind: "offline", message: "", title: "", retrying: false })).toEqual(["reconnect", "rename", "remove"]);
    expect(actions({ kind: "paused" })).toEqual(["reconnect", "rename", "remove"]);
    expect(actions({ kind: "fingerprint", message: "", compare: null })).toEqual(["reconnect", "rename", "remove"]);
    expect(actions({ kind: "connecting" })).toEqual(["rename", "remove"]);
  });
  it("글자와 삭제의 위험 표시", () => {
    const entries = serverMenu({ kind: "connected" });
    expect(entries.map((e) => e.label)).toEqual(["연결 끊기", "이름 변경", "삭제"]);
    expect(entries.find((e) => e.action === "remove")?.danger).toBe(true);
  });
});

describe("연결 목록", () => {
  it("addConnected: 맨 끝에, 중복 없이, 원본을 바꾸지 않는다", () => {
    const keys = ["a"];
    expect(addConnected(keys, "b")).toEqual(["a", "b"]);
    expect(addConnected(["a", "b"], "a")).toEqual(["a", "b"]);
    expect(keys).toEqual(["a"]);
    expect(addConnected(keys, "a")).not.toBe(keys);
  });

  it("removeKey: 없는 키는 그대로", () => {
    expect(removeKey(["a", "b"], "b")).toEqual(["a"]);
    expect(removeKey(["a"], "z")).toEqual(["a"]);
  });

  it("mergeBackend: 백엔드가 기준(아는 순서 유지, 없어진 것은 빼고, 새 것은 끝에)", () => {
    expect(mergeBackend(["a"], ["a", "b", "c"])).toEqual(["a", "b", "c"]);
    expect(mergeBackend(["a", "b"], ["b", "a"])).toEqual(["a", "b"]);
    expect(mergeBackend(["a", "b"], ["a"])).toEqual(["a"]);
    expect(mergeBackend(["a", "b"], ["c"])).toEqual(["c"]);
    expect(mergeBackend([], [])).toEqual([]);
  });
});
