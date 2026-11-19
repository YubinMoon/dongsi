import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ServerLive, ServerView } from "$lib/types";

const mocks = vi.hoisted(() => ({
  connectServer: vi.fn(),
  disconnectServer: vi.fn(),
  connectedServers: vi.fn(),
  listServers: vi.fn(),
  handlers: {} as Record<string, (e: { payload: unknown }) => void>,
}));

vi.mock("$lib/api", () => ({
  api: {
    connectServer: mocks.connectServer,
    disconnectServer: mocks.disconnectServer,
    connectedServers: mocks.connectedServers,
    listServers: mocks.listServers,
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: (e: { payload: unknown }) => void) => {
    mocks.handlers[name] = cb;
    return () => {};
  },
}));

import { conn, connect, disconnect, initConnections, isConnected, setViewing } from "./connections.svelte";
import { refreshServers, serverStore } from "./servers.svelte";

const live = (key: string): ServerLive => ({
  key,
  serverName: key,
  canPublish: true,
  hostOnly: false,
  fingerprint: { short: "ab", full: "abcd" },
  broadcasts: [],
});
const saved = (id: string, withFp: boolean): ServerView => ({
  id,
  name: id,
  addrs: ["1.2.3.4"],
  fingerprint: withFp ? { short: "ab", full: "abcd" } : null,
  savedPasswords: 0,
});

beforeEach(() => {
  conn.tabs = { order: [], closed: [] };
  conn.viewing = null;
  for (const k of Object.keys(conn.card)) delete conn.card[k];
  for (const k of Object.keys(conn.live)) delete conn.live[k];
  for (const k of Object.keys(conn.closedReason)) delete conn.closedReason[k];
  mocks.connectServer.mockReset();
  mocks.disconnectServer.mockReset().mockResolvedValue(undefined);
  mocks.connectedServers.mockReset().mockResolvedValue([]);
  mocks.listServers.mockReset().mockResolvedValue([]);
  vi.stubGlobal("location", { hash: "" });
});

describe("connect", () => {
  it("성공하면 탭과 live를 채우고 카드를 지운다", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    const result = await connect("a");
    expect(result?.key).toBe("a");
    expect(conn.tabs.order).toEqual(["a"]);
    expect(conn.live.a.key).toBe("a");
    expect(conn.card.a).toBeUndefined();
    expect(mocks.listServers).not.toHaveBeenCalled();
  });

  it("지문이 아직 없던 서버는 성공 뒤 목록을 다시 읽는다", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    mocks.listServers.mockResolvedValue([saved("a", true)]);
    serverStore.list = [saved("a", false)];
    await connect("a");
    expect(mocks.listServers).toHaveBeenCalledTimes(1);
    expect(serverStore.list[0].fingerprint).not.toBeNull();
  });

  it("같은 key를 동시에 부르면 하나로 합친다", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    const [x, y] = await Promise.all([connect("a"), connect("a")]);
    expect(mocks.connectServer).toHaveBeenCalledTimes(1);
    expect(x).toBe(y);
  });

  it("연결 중에는 카드가 connecting", async () => {
    let resolve!: (v: ServerLive) => void;
    mocks.connectServer.mockReturnValue(new Promise<ServerLive>((r) => (resolve = r)));
    serverStore.list = [saved("a", true)];
    const p = connect("a");
    expect(conn.card.a).toEqual({ kind: "connecting" });
    resolve(live("a"));
    await p;
    expect(conn.card.a).toBeUndefined();
  });

  it("일반 실패는 failed 카드, 탭 없음, null", async () => {
    mocks.connectServer.mockRejectedValue({ message: "연결하지 못했습니다" });
    serverStore.list = [saved("a", true)];
    expect(await connect("a")).toBeNull();
    expect(conn.card.a).toEqual({ kind: "failed", message: "연결하지 못했습니다" });
    expect(conn.tabs.order).toEqual([]);
  });

  it("지문 불일치는 detail의 두 지문을 카드에 담는다", async () => {
    const stored = { short: "aaaa", full: "aaaa1111" };
    const actual = { short: "bbbb", full: "bbbb2222" };
    mocks.connectServer.mockRejectedValue({
      message: "지문이 다릅니다",
      code: "fingerprintMismatch",
      detail: { stored, actual },
    });
    serverStore.list = [saved("a", true)];
    await connect("a");
    expect(conn.card.a).toEqual({ kind: "fingerprint", message: "지문이 다릅니다", stored, actual });
  });

  it("지문 불일치인데 detail이 없으면 저장된 지문만 담는다", async () => {
    mocks.connectServer.mockRejectedValue({ message: "지문이 다릅니다", code: "fingerprintMismatch" });
    serverStore.list = [saved("a", true)];
    await connect("a");
    expect(conn.card.a).toEqual({
      kind: "fingerprint",
      message: "지문이 다릅니다",
      stored: { short: "ab", full: "abcd" },
      actual: null,
    });
  });
});

describe("disconnect", () => {
  it("탭과 부속 상태를 지우고 백엔드를 부른다", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    await disconnect("a");
    expect(conn.tabs.order).toEqual([]);
    expect(conn.live.a).toBeUndefined();
    expect(mocks.disconnectServer).toHaveBeenCalledWith("a");
  });

  it("보던 서버면 홈으로 간다", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    setViewing("a");
    await disconnect("a");
    expect(location.hash).toBe("#/");
  });

  it("없는 명령이면 조용히 넘어간다", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    mocks.disconnectServer.mockRejectedValue("Command disconnect_server not found");
    serverStore.list = [saved("a", true)];
    await connect("a");
    await expect(disconnect("a")).resolves.toBeUndefined();
    expect(conn.tabs.order).toEqual([]);
  });
});

describe("server-closed", () => {
  it("보고 있는 서버가 끊기면 탭을 남기고 이유를 기록한다", async () => {
    await initConnections();
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    setViewing("a");
    mocks.handlers["server-closed"]({ payload: { key: "a", reason: "서버가 닫혔습니다" } });
    expect(conn.tabs).toEqual({ order: ["a"], closed: ["a"] });
    expect(conn.closedReason.a).toBe("서버가 닫혔습니다");
    expect(isConnected("a")).toBe(false);
    // 다른 화면으로 가면 사라진다
    setViewing(null);
    expect(conn.tabs).toEqual({ order: [], closed: [] });
    expect(conn.closedReason.a).toBeUndefined();
  });

  it("직접 끊은 서버의 뒤늦은 알림은 무시한다", async () => {
    await initConnections();
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    await disconnect("a");
    mocks.handlers["server-closed"]({ payload: { key: "a", reason: "연결을 끊었습니다" } });
    expect(conn.tabs.order).toEqual([]);
    expect(conn.closedReason.a).toBeUndefined();
  });
});

describe("refreshServers", () => {
  it("실패하면 error를 남기고 loaded는 true", async () => {
    mocks.listServers.mockRejectedValue({ message: "읽지 못했습니다" });
    await refreshServers();
    expect(serverStore.error).toBe("읽지 못했습니다");
    expect(serverStore.loaded).toBe(true);
  });
});
