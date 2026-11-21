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

import {
  autoConnectTick,
  conn,
  connect,
  connectSoon,
  disconnect,
  initConnections,
  isConnected,
  resetAutoConnect,
  retryDelay,
  statusOf,
  syncConnected,
} from "./connections.svelte";
import { initServerEvents, refreshServers, serverStore } from "./servers.svelte";

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
  conn.connected = [];
  for (const k of Object.keys(conn.card)) delete conn.card[k];
  for (const k of Object.keys(conn.trying)) delete conn.trying[k];
  for (const k of Object.keys(conn.live)) delete conn.live[k];
  for (const k of Object.keys(conn.closedReason)) delete conn.closedReason[k];
  mocks.connectServer.mockReset();
  mocks.disconnectServer.mockReset().mockResolvedValue(undefined);
  mocks.connectedServers.mockReset().mockResolvedValue([]);
  mocks.listServers.mockReset().mockResolvedValue([]);
  vi.stubGlobal("location", { hash: "" });
  resetAutoConnect();
  serverStore.loaded = true;
  serverStore.error = null;
  serverStore.list = [];
});

describe("connect", () => {
  it("성공하면 연결 목록과 live를 채우고 시도 기록을 지운다", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    const result = await connect("a");
    expect(result?.key).toBe("a");
    expect(conn.connected).toEqual(["a"]);
    expect(statusOf("a")).toEqual({ kind: "connected" });
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

  it("연결 중에는 connecting(탭·서버 화면)", async () => {
    let resolve!: (v: ServerLive) => void;
    mocks.connectServer.mockReturnValue(new Promise<ServerLive>((r) => (resolve = r)));
    serverStore.list = [saved("a", true)];
    const p = connect("a");
    expect(conn.card.a).toEqual({ kind: "connecting" });
    expect(conn.trying.a).toBe(true);
    expect(statusOf("a")).toEqual({ kind: "connecting" });
    resolve(live("a"));
    await p;
    expect(conn.card.a).toBeUndefined();
    expect(conn.trying.a).toBeUndefined();
  });

  it("일반 실패는 failed(빨강), 연결 목록 없음, null", async () => {
    mocks.connectServer.mockRejectedValue({ message: "연결하지 못했습니다" });
    serverStore.list = [saved("a", true)];
    expect(await connect("a")).toBeNull();
    expect(conn.card.a).toEqual({ kind: "failed", message: "연결하지 못했습니다" });
    expect(conn.connected).toEqual([]);
    expect(statusOf("a").kind).toBe("failed");
    expect(conn.trying.a).toBeUndefined();
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
    expect(statusOf("a")).toEqual({ kind: "fingerprint", message: "지문이 다릅니다", compare: { stored, actual } });
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
  it("연결 목록과 live를 지우고 백엔드를 부른다. 상태는 '연결 끊음'(탭은 남는다)", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    await disconnect("a");
    expect(conn.connected).toEqual([]);
    expect(conn.live.a).toBeUndefined();
    expect(mocks.disconnectServer).toHaveBeenCalledWith("a");
    expect(statusOf("a")).toEqual({ kind: "paused" });
  });

  it("보던 서버여도 화면을 옮기지 않는다(서버 화면이 '연결을 끊었습니다'를 보인다)", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    location.hash = "#/server/a";
    await disconnect("a");
    expect(location.hash).toBe("#/server/a");
  });

  it("끊김 이유와 실패 기록도 지운다", async () => {
    conn.closedReason.a = "서버가 닫혔습니다";
    conn.card.a = { kind: "failed", message: "x" };
    await disconnect("a");
    expect(conn.closedReason.a).toBeUndefined();
    expect(conn.card.a).toBeUndefined();
    expect(statusOf("a")).toEqual({ kind: "paused" });
  });

  it("없는 명령이면 조용히 넘어간다", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    mocks.disconnectServer.mockRejectedValue("Command disconnect_server not found");
    serverStore.list = [saved("a", true)];
    await connect("a");
    await expect(disconnect("a")).resolves.toBe(true);
    expect(conn.connected).toEqual([]);
  });

  it("방송 중이라 거절되면 연결을 그대로 두고 false", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    mocks.disconnectServer.mockRejectedValue({
      message: "방송 중인 서버는 연결을 끊을 수 없습니다. 방송을 먼저 끝내 주세요.",
      code: "broadcasting",
    });
    serverStore.list = [saved("a", true)];
    await connect("a");
    location.hash = "#/server/a";
    await expect(disconnect("a")).resolves.toBe(false);
    expect(conn.connected).toEqual(["a"]);
    expect(isConnected("a")).toBe(true);
    expect(location.hash).toBe("#/server/a");
  });

  it("localOnly면 백엔드를 부르지 않고 이쪽 상태만 지운다(서버 삭제)", async () => {
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    await disconnect("a", { localOnly: true });
    expect(mocks.disconnectServer).not.toHaveBeenCalled();
    expect(conn.connected).toEqual([]);
  });

  it("끊는 동안 먼저 온 server-closed는 끊김으로 보이지 않는다", async () => {
    await initConnections();
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    mocks.disconnectServer.mockImplementation(async () => {
      mocks.handlers["server-closed"]({ payload: { key: "a", reason: "연결을 끊었습니다" } });
    });
    await disconnect("a");
    expect(conn.connected).toEqual([]);
    expect(conn.closedReason.a).toBeUndefined();
    expect(statusOf("a")).toEqual({ kind: "paused" });
  });
});

describe("server-closed", () => {
  it("서버가 끊기면 연결 목록에서 빼고 이유를 남긴다(closed). 다시 연결되면 이유를 지운다", async () => {
    await initConnections();
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    mocks.handlers["server-closed"]({ payload: { key: "a", reason: "서버가 닫혔습니다" } });
    expect(conn.connected).toEqual([]);
    expect(conn.closedReason.a).toBe("서버가 닫혔습니다");
    expect(isConnected("a")).toBe(false);
    expect(statusOf("a")).toEqual({ kind: "closed", reason: "서버가 닫혔습니다", retrying: false });
    await connect("a");
    expect(conn.closedReason.a).toBeUndefined();
    expect(statusOf("a").kind).toBe("connected");
  });

  it("끊긴 뒤 자동 재시도가 실패해도 '끊어졌습니다'를 그대로 보인다", async () => {
    await initConnections();
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    mocks.handlers["server-closed"]({ payload: { key: "a", reason: "서버가 닫혔습니다" } });
    mocks.connectServer.mockRejectedValue({ message: "연결 거부" });
    await connect("a", { quiet: true });
    expect(conn.card.a).toEqual({ kind: "offline", message: "연결 거부" });
    expect(statusOf("a").kind).toBe("closed");
  });

  it("직접 끊은 서버의 뒤늦은 알림은 무시한다", async () => {
    await initConnections();
    mocks.connectServer.mockResolvedValue(live("a"));
    serverStore.list = [saved("a", true)];
    await connect("a");
    await disconnect("a");
    mocks.handlers["server-closed"]({ payload: { key: "a", reason: "연결을 끊었습니다" } });
    expect(conn.connected).toEqual([]);
    expect(conn.closedReason.a).toBeUndefined();
  });
});

describe("자동 연결(D-071)", () => {
  /** 기다리는 약속을 모두 흘려보낸다. */
  const flush = () => new Promise((r) => setTimeout(r, 0));

  it("retryDelay: 5초에서 두 배씩, 최대 60초", () => {
    expect([1, 2, 3, 4, 5, 6, 9].map(retryDelay)).toEqual([5000, 10000, 20000, 40000, 60000, 60000, 60000]);
  });

  it("quiet 연결은 시도 기록을 남기지 않지만 처음에는 connecting으로 보이고, 성공하면 연결 목록과 live를 채운다", async () => {
    let resolve!: (v: ServerLive) => void;
    mocks.connectServer.mockReturnValue(new Promise<ServerLive>((r) => (resolve = r)));
    serverStore.list = [saved("a", true)];
    const p = connect("a", { quiet: true });
    expect(conn.card.a).toBeUndefined();
    expect(statusOf("a")).toEqual({ kind: "connecting" });
    resolve(live("a"));
    await p;
    expect(conn.connected).toEqual(["a"]);
    expect(conn.live.a.key).toBe("a");
  });

  it("quiet 연결 실패는 offline(사용자가 직접 실패한 failed는 그대로 둔다)", async () => {
    mocks.connectServer.mockRejectedValue({ message: "서버가 응답하지 않습니다" });
    serverStore.list = [saved("a", true)];
    expect(await connect("a", { quiet: true })).toBeNull();
    expect(conn.card.a).toEqual({ kind: "offline", message: "서버가 응답하지 않습니다" });
    expect(statusOf("a").kind).toBe("offline");
    await connect("a");
    expect(conn.card.a).toEqual({ kind: "failed", message: "서버가 응답하지 않습니다" });
    await connect("a", { quiet: true });
    expect(conn.card.a.kind).toBe("failed");
  });

  it("offline 서버를 다시 시도하는 동안에도 offline으로 보인다(탭이 깜빡이지 않게)", async () => {
    mocks.connectServer.mockRejectedValue({ message: "서버가 응답하지 않습니다" });
    serverStore.list = [saved("a", true)];
    await connect("a", { quiet: true });
    let reject!: (e: unknown) => void;
    mocks.connectServer.mockReturnValue(new Promise<ServerLive>((_, r) => (reject = r)));
    const p = connect("a", { quiet: true });
    expect(statusOf("a")).toMatchObject({ kind: "offline", retrying: true });
    reject({ message: "서버가 응답하지 않습니다" });
    await p;
    expect(statusOf("a")).toMatchObject({ kind: "offline", retrying: false });
  });

  it("자동 연결 중에 '다시 연결'을 누르면 같은 시도를 기다리고, 실패는 failed로 보인다", async () => {
    let reject!: (e: unknown) => void;
    mocks.connectServer.mockReturnValue(new Promise<ServerLive>((_, r) => (reject = r)));
    serverStore.list = [saved("a", true)];
    const quiet = connect("a", { quiet: true });
    const clicked = connect("a");
    expect(conn.card.a).toEqual({ kind: "connecting" });
    reject({ message: "접속하지 못했습니다" });
    expect(await clicked).toBeNull();
    await quiet;
    expect(mocks.connectServer).toHaveBeenCalledTimes(1);
    expect(conn.card.a).toEqual({ kind: "failed", message: "접속하지 못했습니다" });
  });

  it("목록을 읽기 전에는 하지 않고, 읽은 뒤에는 연결 안 된 저장한 서버에 모두 연결한다", async () => {
    mocks.connectServer.mockImplementation(async (key: string) => live(key));
    serverStore.list = [saved("a", true), saved("b", true)];
    serverStore.loaded = false;
    autoConnectTick();
    expect(mocks.connectServer).not.toHaveBeenCalled();
    serverStore.loaded = true;
    autoConnectTick();
    await flush();
    expect(mocks.connectServer.mock.calls.map((c) => c[0])).toEqual(["a", "b"]);
    expect(isConnected("a") && isConnected("b")).toBe(true);
    // 이미 연결된 서버는 다시 부르지 않는다
    autoConnectTick();
    expect(mocks.connectServer).toHaveBeenCalledTimes(2);
  });

  it("실패한 서버는 retryDelay가 지난 뒤에만 다시 시도한다", async () => {
    mocks.connectServer.mockRejectedValue({ message: "연결 거부" });
    serverStore.list = [saved("a", true)];
    autoConnectTick();
    await flush();
    expect(mocks.connectServer).toHaveBeenCalledTimes(1);
    autoConnectTick(Date.now() + 1000);
    expect(mocks.connectServer).toHaveBeenCalledTimes(1);
    autoConnectTick(Date.now() + retryDelay(1) + 10);
    await flush();
    expect(mocks.connectServer).toHaveBeenCalledTimes(2);
    // 두 번째 실패 뒤에는 10초
    autoConnectTick(Date.now() + retryDelay(1) + 10);
    expect(mocks.connectServer).toHaveBeenCalledTimes(2);
    autoConnectTick(Date.now() + retryDelay(2) + 10);
    await flush();
    expect(mocks.connectServer).toHaveBeenCalledTimes(3);
  });

  it("connectSoon은 기다리던 서버를 바로 다시 시도한다", async () => {
    mocks.connectServer.mockRejectedValue({ message: "연결 거부" });
    serverStore.list = [saved("a", true), saved("b", true)];
    autoConnectTick();
    await flush();
    expect(mocks.connectServer).toHaveBeenCalledTimes(2);
    connectSoon(["b"]);
    await flush();
    expect(mocks.connectServer.mock.calls.map((c) => c[0])).toEqual(["a", "b", "b"]);
  });

  it("지문 경고가 있는 서버는 자동으로 다시 시도하지 않는다", async () => {
    mocks.connectServer.mockRejectedValue({ message: "지문이 다릅니다", code: "fingerprintMismatch" });
    serverStore.list = [saved("a", true)];
    autoConnectTick();
    await flush();
    expect(conn.card.a.kind).toBe("fingerprint");
    autoConnectTick(Date.now() + 3_600_000);
    connectSoon();
    expect(mocks.connectServer).toHaveBeenCalledTimes(1);
  });

  it("사용자가 끊은 서버는 자동 연결하지 않고, '다시 연결'로 연결하면 다시 대상이 된다", async () => {
    mocks.connectServer.mockImplementation(async (key: string) => live(key));
    serverStore.list = [saved("a", true)];
    await connect("a");
    await disconnect("a");
    autoConnectTick(Date.now() + 3_600_000);
    connectSoon();
    expect(mocks.connectServer).toHaveBeenCalledTimes(1);
    await connect("a");
    expect(isConnected("a")).toBe(true);
    mocks.connectServer.mockClear();
    // 다시 끊기지 않고 서버 쪽에서 끊기면 자동으로 다시 연결된다
    await initConnections();
    mocks.handlers["server-closed"]({ payload: { key: "a", reason: "서버가 닫혔습니다" } });
    expect(isConnected("a")).toBe(false);
    autoConnectTick();
    expect(mocks.connectServer).not.toHaveBeenCalled();
    autoConnectTick(Date.now() + 2500);
    await flush();
    expect(mocks.connectServer).toHaveBeenCalledWith("a");
    expect(isConnected("a")).toBe(true);
  });

  it("방송 중이라 끊지 못하면 자동 연결 대상으로 남는다", async () => {
    mocks.connectServer.mockImplementation(async (key: string) => live(key));
    mocks.disconnectServer.mockRejectedValue({ message: "방송 중", code: "broadcasting" });
    serverStore.list = [saved("a", true)];
    await connect("a");
    await disconnect("a");
    await initConnections();
    mocks.handlers["server-closed"]({ payload: { key: "a", reason: "서버가 닫혔습니다" } });
    mocks.connectServer.mockClear();
    autoConnectTick(Date.now() + 2500);
    await flush();
    expect(mocks.connectServer).toHaveBeenCalledWith("a");
  });

  it("자동 연결을 기다리는 사이에 끊으면 결과를 버리고 늦게 열린 연결도 닫는다", async () => {
    let resolve!: (v: ServerLive) => void;
    mocks.connectServer.mockReturnValue(new Promise<ServerLive>((r) => (resolve = r)));
    serverStore.list = [saved("a", true)];
    const p = connect("a", { quiet: true });
    await disconnect("a");
    mocks.disconnectServer.mockClear();
    resolve(live("a"));
    expect(await p).toBeNull();
    expect(isConnected("a")).toBe(false);
    expect(conn.live.a).toBeUndefined();
    expect(mocks.disconnectServer).toHaveBeenCalledWith("a");
  });
});

describe("syncConnected", () => {
  it("백엔드 목록에 맞추고, 백엔드에서 사라진 연결은 곧 다시 자동 연결한다", async () => {
    mocks.connectServer.mockImplementation(async (key: string) => live(key));
    serverStore.list = [saved("a", true), saved("b", true)];
    await connect("a");
    mocks.connectedServers.mockResolvedValue(["b"]);
    await syncConnected();
    expect(conn.connected).toEqual(["b"]);
    expect(conn.live.a).toBeUndefined();
    mocks.connectServer.mockClear();
    autoConnectTick(Date.now() + 2500);
    await flushAll();
    expect(mocks.connectServer).toHaveBeenCalledWith("a");
  });
});

const flushAll = () => new Promise((r) => setTimeout(r, 0));

describe("servers-changed", () => {
  it("이벤트가 오면 목록을 다시 읽는다", async () => {
    await initServerEvents();
    mocks.listServers.mockResolvedValue([saved("new", true)]);
    mocks.handlers["servers-changed"]({ payload: {} });
    await vi.waitFor(() => expect(serverStore.list.map((s) => s.id)).toEqual(["new"]));
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
