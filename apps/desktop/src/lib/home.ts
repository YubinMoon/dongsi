import { formatAge, qualityLabel, secondsSince, viaLabel } from "./format";
import { watchHref } from "./serverScreen";
import type { LocalServerView, ServerLive, ServerView } from "./types";

/** 홈 "방송 중" 격자의 타일 하나(`BroadcastTile` 입력). */
export interface LiveTile {
  /** `<서버 key>/<방송자 ID>`. 같은 방송이면 감지 결과로 보이든 서버 목록으로 보이든 같다. */
  key: string;
  seed: string;
  name: string;
  /** 로컬 감지로 찾은 경로("LAN", "Tailscale", "이 PC"). 감지되지 않은 서버의 방송이면 null. */
  via: string | null;
  /** 방송 시간("30분"). 연결한 서버의 방송만 안다(감지 응답에는 시작 시각이 없다). */
  elapsed: string | null;
  /** 시청자 수. 연결한 서버의 방송만 안다. */
  viewers: number | null;
  locked: boolean;
  /** 저장한 서버에 이 방송자의 비밀번호를 기억해 뒀다(잠긴 방송에서만 뜻이 있다). */
  passwordSaved: boolean;
  /** "서버 이름 · 1080p · 60fps" */
  meta: string;
  href: string;
}

function tileMetaLine(serverName: string, b: { height: number; fps: number }): string {
  return `${serverName} · ${qualityLabel(b.height, b.fps)}`;
}

function localServerTiles(server: LocalServerView, name: string): LiveTile[] {
  return server.broadcasts.map((b) => ({
    key: `${server.key}/${b.broadcasterId}`,
    seed: b.broadcasterId,
    name: b.broadcasterName,
    via: viaLabel(server.via),
    elapsed: null,
    viewers: null,
    locked: b.locked,
    passwordSaved: b.passwordSaved,
    meta: tileMetaLine(name, b),
    href: watchHref(server.key, b.broadcasterId),
  }));
}

/**
 * 홈 "방송 중" 목록(D-071): 연결한 저장한 서버의 방송과 로컬 감지(LAN·Tailscale)로 찾은 방송을 한 목록으로.
 *
 * - `servers`는 저장한 서버(순서 그대로), `live`는 **연결돼 있는** 서버의 상태(key = 서버 ID), `local`은 감지 결과(null이면 아직 못 찾음).
 * - 순서: 저장한 서버 순서, 그 뒤에 저장하지 않은 감지 서버(감지 순서). 서버 안에서는 서버가 보낸 순서.
 * - 연결한 서버는 서버가 보낸 목록을 쓴다(시청자 수·방송 시간이 있다). 연결 전이지만 감지된 저장한 서버는 감지 결과를 쓴다.
 *   같은 서버를 두 곳에서 알면 연결한 쪽만 쓰고, 감지된 경로(LAN 등)만 빌려 온다.
 * - 이름은 저장한 서버면 사용자가 붙인 이름, 아니면 서버가 알린 이름.
 * - 내 방송은 뺀다(D-065: 이 PC에서 시청하지 않는다. 감지 결과에서는 백엔드가 이미 뺐다).
 */
export function liveTiles(
  servers: Pick<ServerView, "id" | "name">[],
  live: Record<string, ServerLive | undefined>,
  local: LocalServerView[] | null,
  nowMs: number,
): LiveTile[] {
  const found = new Map((local ?? []).map((s) => [s.key, s]));
  const tiles: LiveTile[] = [];
  const savedIds = new Set<string>();
  for (const server of servers) {
    savedIds.add(server.id);
    const state = live[server.id];
    const seen = found.get(server.id);
    if (state) {
      const via = seen ? viaLabel(seen.via) : null;
      for (const b of state.broadcasts) {
        if (b.mine) continue;
        tiles.push({
          key: `${server.id}/${b.broadcasterId}`,
          seed: b.broadcasterId,
          name: b.broadcasterName,
          via,
          elapsed: formatAge(secondsSince(b.startedAt, nowMs)),
          viewers: b.viewers,
          locked: b.locked,
          passwordSaved: b.passwordSaved,
          meta: tileMetaLine(server.name, b),
          href: watchHref(server.id, b.broadcasterId),
        });
      }
    } else if (seen) {
      tiles.push(...localServerTiles(seen, server.name));
    }
  }
  for (const server of local ?? []) {
    if (!savedIds.has(server.key)) tiles.push(...localServerTiles(server, server.name));
  }
  return tiles;
}
