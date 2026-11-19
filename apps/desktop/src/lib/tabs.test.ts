import { describe, expect, it } from "vitest";
import {
  addConnected,
  computeTabs,
  mergeBackend,
  onServerClosed,
  onViewingChanged,
  removeKey,
  type TabState,
} from "./tabs";
import type { ServerView } from "./types";

const st = (order: string[], closed: string[] = []): TabState => ({ order, closed });
const server = (id: string, name = id): ServerView => ({
  id,
  name,
  addrs: [],
  fingerprint: null,
  savedPasswords: 0,
});

describe("addConnected", () => {
  it("맨 끝에 더한다", () => {
    expect(addConnected(st(["a"]), "b")).toEqual(st(["a", "b"]));
    expect(addConnected(st([]), "a")).toEqual(st(["a"]));
  });

  it("이미 있으면 중복 없이 그대로", () => {
    expect(addConnected(st(["a", "b"]), "a")).toEqual(st(["a", "b"]));
  });

  it("끊긴 채 남은 탭은 그 자리에서 되살린다", () => {
    expect(addConnected(st(["a", "b", "c"], ["b"]), "b")).toEqual(st(["a", "b", "c"]));
  });

  it("원본을 바꾸지 않는다", () => {
    const s = st(["a"], []);
    const next = addConnected(s, "b");
    expect(s).toEqual(st(["a"]));
    expect(next).not.toBe(s);
    expect(next.order).not.toBe(s.order);
  });
});

describe("removeKey", () => {
  it("order와 closed에서 모두 지운다", () => {
    expect(removeKey(st(["a", "b"], ["b"]), "b")).toEqual(st(["a"]));
  });

  it("없는 키는 그대로", () => {
    expect(removeKey(st(["a"]), "z")).toEqual(st(["a"]));
  });
});

describe("onServerClosed", () => {
  it("보고 있는 서버면 끊긴 탭으로 남긴다", () => {
    expect(onServerClosed(st(["a", "b"]), "b", "b")).toEqual(st(["a", "b"], ["b"]));
  });

  it("보고 있지 않으면 지운다", () => {
    expect(onServerClosed(st(["a", "b"]), "b", "a")).toEqual(st(["a"]));
    expect(onServerClosed(st(["a", "b"]), "b", null)).toEqual(st(["a"]));
  });

  it("order에 없던 키(이미 직접 끊은 것)는 그대로", () => {
    expect(onServerClosed(st(["a"]), "z", "z")).toEqual(st(["a"]));
    expect(onServerClosed(st(["a"]), "z", null)).toEqual(st(["a"]));
  });

  it("두 번 와도 closed에 한 번만", () => {
    const once = onServerClosed(st(["a"]), "a", "a");
    expect(onServerClosed(once, "a", "a")).toEqual(st(["a"], ["a"]));
  });
});

describe("onViewingChanged", () => {
  it("closed 중 보는 서버가 아닌 것을 지운다", () => {
    expect(onViewingChanged(st(["a", "b", "c"], ["b"]), "a")).toEqual(st(["a", "c"]));
    expect(onViewingChanged(st(["a", "b", "c"], ["b"]), null)).toEqual(st(["a", "c"]));
  });

  it("보고 있는 끊긴 탭은 남긴다", () => {
    expect(onViewingChanged(st(["a", "b"], ["b"]), "b")).toEqual(st(["a", "b"], ["b"]));
  });

  it("closed가 없으면 그대로", () => {
    expect(onViewingChanged(st(["a", "b"]), "a")).toEqual(st(["a", "b"]));
  });
});

describe("mergeBackend", () => {
  it("백엔드에만 있는 키는 끝에 더한다", () => {
    expect(mergeBackend(st(["a"]), ["a", "b", "c"], null)).toEqual(st(["a", "b", "c"]));
  });

  it("탭에는 있는데 백엔드에 없으면 보고 있을 때만 끊긴 탭으로 남긴다", () => {
    expect(mergeBackend(st(["a", "b"]), ["a"], "b")).toEqual(st(["a", "b"], ["b"]));
    expect(mergeBackend(st(["a", "b"]), ["a"], "a")).toEqual(st(["a"]));
    expect(mergeBackend(st(["a", "b"]), ["a"], null)).toEqual(st(["a"]));
  });

  it("이미 끊긴 탭은 건드리지 않는다", () => {
    expect(mergeBackend(st(["a", "b"], ["b"]), ["a"], "b")).toEqual(st(["a", "b"], ["b"]));
  });

  it("같으면 그대로, 둘 다 비어도 된다", () => {
    expect(mergeBackend(st(["a", "b"]), ["b", "a"], null)).toEqual(st(["a", "b"]));
    expect(mergeBackend(st([]), [], null)).toEqual(st([]));
  });

  it("원본을 바꾸지 않는다", () => {
    const s = st(["a", "b"]);
    mergeBackend(s, ["a", "c"], "b");
    expect(s).toEqual(st(["a", "b"]));
  });
});

describe("computeTabs", () => {
  const servers = [server("a", "우리집"), server("b", "민수 PC"), server("c", "회사")];

  it("order 순서대로, 저장한 서버만", () => {
    const tabs = computeTabs(st(["c", "x", "a"]), servers, null);
    expect(tabs.map((t) => t.id)).toEqual(["c", "a"]);
    expect(tabs.map((t) => t.name)).toEqual(["회사", "우리집"]);
  });

  it("active와 closed 표시", () => {
    const tabs = computeTabs(st(["a", "b"], ["b"]), servers, "b");
    expect(tabs).toEqual([
      { id: "a", name: "우리집", closed: false, active: false },
      { id: "b", name: "민수 PC", closed: true, active: true },
    ]);
  });

  it("비어 있으면 빈 배열", () => {
    expect(computeTabs(st([]), servers, null)).toEqual([]);
    expect(computeTabs(st(["a"]), [], null)).toEqual([]);
  });
});
