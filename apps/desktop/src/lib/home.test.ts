import { describe, expect, it } from "vitest";
import { liveTiles } from "./home";
import type { BroadcastView, LocalServerView, ServerLive } from "./types";

const fp = (short: string) => ({ short, full: short + ":FULL" });

describe("liveTiles", () => {
  const NOW = 1_800_000_000_000;
  const nowSec = NOW / 1000;
  const local: LocalServerView[] = [
    {
      key: "local-1",
      name: "준호의 PC",
      addr: "192.168.0.5:7884",
      via: "lan",
      hostOnly: true,
      saved: false,
      broadcasts: [
        { broadcasterId: "b1", broadcasterName: "준호", codec: "h264", width: 1920, height: 1080, fps: 60, locked: false, passwordSaved: false },
      ],
    },
    {
      key: "s1",
      name: "유나 노트북(알린 이름)",
      addr: "100.1.1.1:7884",
      via: "tailscale",
      hostOnly: false,
      saved: true,
      broadcasts: [
        { broadcasterId: "b2", broadcasterName: "유나", codec: "av1", width: 2560, height: 1440, fps: 60, locked: true, passwordSaved: true },
        { broadcasterId: "b3", broadcasterName: "민수", codec: "av1", width: 3840, height: 2160, fps: 30, locked: false, passwordSaved: false },
      ],
    },
  ];
  const saved = [
    { id: "s0", name: "우리집 서버" },
    { id: "s1", name: "유나 노트북" },
  ];
  const view = (id: string, name: string, extra: Partial<BroadcastView> = {}): BroadcastView => ({
    broadcasterId: id,
    broadcasterName: name,
    codec: "h264",
    width: 1920,
    height: 1080,
    fps: 60,
    hasAudio: true,
    locked: false,
    viewers: 2,
    startedAt: nowSec - 30 * 60,
    passwordSaved: false,
    mine: false,
    ...extra,
  });
  const live = (key: string, broadcasts: BroadcastView[]): ServerLive => ({
    key,
    serverName: key,
    canPublish: true,
    hostOnly: false,
    fingerprint: fp("A"),
    broadcasts,
  });

  it("감지만 있으면 감지 결과로 타일을 만든다(저장한 서버는 사용자가 붙인 이름)", () => {
    const tiles = liveTiles(saved, {}, local, NOW);
    // 저장한 서버(s1)가 저장 순서대로 먼저, 저장하지 않은 감지 서버(local-1)가 뒤
    expect(tiles.map((t) => t.key)).toEqual(["s1/b2", "s1/b3", "local-1/b1"]);
    expect(tiles[2]).toEqual({
      key: "local-1/b1",
      seed: "b1",
      name: "준호",
      via: "LAN",
      elapsed: null,
      viewers: null,
      locked: false,
      passwordSaved: false,
      meta: "준호의 PC · 1080p · 60fps · H.264",
      href: "#/watch/local-1/b1",
    });
    expect(tiles[0]).toMatchObject({ via: "Tailscale", locked: true, passwordSaved: true, meta: "유나 노트북 · 1440p · 60fps · AV1" });
    expect(tiles[1].meta).toBe("유나 노트북 · 4K · 30fps · AV1");
  });

  it("연결한 서버는 서버 목록을 쓰고(시청자·방송 시간), 같은 서버의 감지 결과는 경로만 빌린다", () => {
    const tiles = liveTiles(
      saved,
      {
        s0: live("s0", [view("h1", "하린", { locked: true, passwordSaved: true, viewers: 3 })]),
        s1: live("s1", [view("b2", "유나", { codec: "av1", height: 1440 })]),
      },
      local,
      NOW,
    );
    expect(tiles.map((t) => t.key)).toEqual(["s0/h1", "s1/b2", "local-1/b1"]);
    expect(tiles[0]).toEqual({
      key: "s0/h1",
      seed: "h1",
      name: "하린",
      via: null,
      elapsed: "30분",
      viewers: 3,
      locked: true,
      passwordSaved: true,
      meta: "우리집 서버 · 1080p · 60fps · H.264",
      href: "#/watch/s0/h1",
    });
    // 감지 결과의 b3(민수)는 서버 목록에 없으니 보이지 않는다(서버 목록이 기준)
    expect(tiles[1]).toMatchObject({ via: "Tailscale", viewers: 2, meta: "유나 노트북 · 1440p · 60fps · AV1" });
  });

  it("내 방송은 뺀다", () => {
    const tiles = liveTiles(saved, { s0: live("s0", [view("me", "나", { mine: true }), view("h1", "하린")]) }, null, NOW);
    expect(tiles.map((t) => t.name)).toEqual(["하린"]);
  });

  it("연결 안 된 서버와 감지 안 된 서버는 없다, 아무것도 없으면 빈 목록", () => {
    expect(liveTiles(saved, {}, null, NOW)).toEqual([]);
    expect(liveTiles(saved, {}, [{ ...local[0], broadcasts: [] }], NOW)).toEqual([]);
    expect(liveTiles([], { s9: live("s9", [view("x", "엑스")]) }, null, NOW)).toEqual([]);
  });
});
