import { describe, expect, it } from "vitest";
import { cardClickAction, cardStatus, countLabel, euro, fingerprintPrefix, isHomeHash, localTiles } from "./home";
import type { LocalServerView } from "./types";

const fp = (short: string) => ({ short, full: short + ":FULL" });
const known = { fingerprint: fp("3A:9F"), savedPasswords: 0 };

describe("cardStatus", () => {
  it("접속 시도 상태가 저장된 상태보다 우선한다", () => {
    expect(cardStatus(known, true, { kind: "connecting" })).toEqual({ kind: "connecting" });
    expect(cardStatus(known, false, { kind: "failed", message: "접속하지 못했습니다" })).toEqual({
      kind: "failed",
      message: "접속하지 못했습니다",
    });
  });

  it("지문 불일치: 저장·실제 지문이 다 있으면 비교, 하나라도 없으면 message만", () => {
    const both = cardStatus(known, false, {
      kind: "fingerprint",
      message: "다릅니다",
      stored: fp("A"),
      actual: fp("B"),
    });
    expect(both).toEqual({ kind: "fingerprint", message: "다릅니다", compare: { stored: fp("A"), actual: fp("B") } });
    const noActual = cardStatus(known, false, {
      kind: "fingerprint",
      message: "다릅니다",
      stored: fp("A"),
      actual: null,
    });
    expect(noActual).toEqual({ kind: "fingerprint", message: "다릅니다", compare: null });
  });

  it("연결됨: 지문과 비밀번호 개수를 함께 돌려준다", () => {
    expect(cardStatus({ fingerprint: fp("A"), savedPasswords: 2 }, true, undefined)).toEqual({
      kind: "connected",
      fingerprinted: true,
      savedPasswords: 2,
    });
    expect(cardStatus({ fingerprint: null, savedPasswords: 0 }, true, undefined)).toEqual({
      kind: "connected",
      fingerprinted: false,
      savedPasswords: 0,
    });
  });

  it("연결 안 됨: 지문이 있으면 known, 없으면 첫 접속 전", () => {
    expect(cardStatus({ fingerprint: fp("A"), savedPasswords: 1 }, false, undefined)).toEqual({
      kind: "known",
      savedPasswords: 1,
    });
    expect(cardStatus({ fingerprint: null, savedPasswords: 0 }, false, undefined)).toEqual({ kind: "firstTime" });
  });
});

describe("cardClickAction", () => {
  it("연결돼 있으면 열고, 아니면 연결하고, 접속 중이면 아무것도 안 한다", () => {
    expect(cardClickAction(true, undefined)).toBe("open");
    expect(cardClickAction(false, undefined)).toBe("connect");
    expect(cardClickAction(false, { kind: "failed", message: "x" })).toBe("connect");
    expect(cardClickAction(false, { kind: "connecting" })).toBe("none");
  });
});

describe("isHomeHash", () => {
  it("홈 해시만 참", () => {
    for (const h of ["", "#", "#/", "#/?x=1"]) expect(isHomeHash(h)).toBe(true);
    for (const h of ["#/server/a", "#/settings", "#/broadcast"]) expect(isHomeHash(h)).toBe(false);
  });
});

describe("localTiles", () => {
  const servers: LocalServerView[] = [
    {
      key: "local-1",
      name: "준호의 PC",
      addr: "192.168.0.5:7884",
      via: "lan",
      hostOnly: true,
      saved: false,
      broadcasts: [
        { broadcasterId: "b1", broadcasterName: "준호", codec: "h264", width: 1920, height: 1080, fps: 60, locked: false },
      ],
    },
    {
      key: "s1",
      name: "유나 노트북",
      addr: "100.1.1.1:7884",
      via: "tailscale",
      hostOnly: false,
      saved: true,
      broadcasts: [
        { broadcasterId: "b2", broadcasterName: "유나", codec: "vp9", width: 2560, height: 1440, fps: 60, locked: true },
        { broadcasterId: "b3", broadcasterName: "민수", codec: "av1", width: 3840, height: 2160, fps: 30, locked: false },
      ],
    },
  ];

  it("방송마다 타일을 만든다", () => {
    const tiles = localTiles(servers);
    expect(tiles).toHaveLength(3);
    expect(tiles[0]).toEqual({
      key: "local-1/b1",
      seed: "b1",
      name: "준호",
      via: "LAN",
      locked: false,
      meta: "준호의 PC · 1080p · 60fps · H.264",
      href: "#/watch/local-1/b1",
    });
    expect(tiles[1]).toMatchObject({ via: "Tailscale", locked: true, meta: "유나 노트북 · 1440p · 60fps · VP9" });
    expect(tiles[2].meta).toBe("유나 노트북 · 4K · 30fps · AV1");
  });

  it("null이거나 방송이 없으면 빈 목록", () => {
    expect(localTiles(null)).toEqual([]);
    expect(localTiles([{ ...servers[0], broadcasts: [] }])).toEqual([]);
  });
});

describe("countLabel", () => {
  it("개수", () => expect(countLabel(3)).toBe("3개"));
});

describe("euro", () => {
  it("받침에 맞는 조사를 고른다", () => {
    expect(euro("우리집 서버")).toBe("로");
    expect(euro("민수 PC")).toBe("(으)로");
    expect(euro("회사 테일넷")).toBe("으로");
    expect(euro("서울")).toBe("로");
    expect(euro("집")).toBe("으로");
    expect(euro("")).toBe("(으)로");
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
