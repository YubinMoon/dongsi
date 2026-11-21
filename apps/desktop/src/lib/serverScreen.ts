import { codecLabel, qualityLabel } from "./format";
import type { BroadcastView } from "./types";

/**
 * 서버 화면이 보여 줄 것.
 * - missing: 서버 목록에 없다(삭제됨)
 * - loading: 서버 목록을 아직 못 읽었다
 * - live: 연결돼 있다(머리 + 방송 목록)
 * - closed: 보던 중에 연결이 끊겨 탭만 남았다(머리 + 다시 연결)
 * - connecting: 연결 안 됐고 끊긴 탭도 아니다(주소로 바로 들어옴 등). 연결을 시도한다.
 */
export type ServerScreen = "missing" | "loading" | "live" | "closed" | "connecting";

export function serverScreen(input: {
  listLoaded: boolean;
  exists: boolean;
  connected: boolean;
  closedTab: boolean;
}): ServerScreen {
  if (!input.exists) return input.listLoaded ? "missing" : "loading";
  if (input.closedTab) return "closed";
  if (input.connected) return "live";
  return "connecting";
}

/** 머리 아래 줄의 주소: "a, b" */
export function addrLine(addrs: string[]): string {
  return addrs.join(", ");
}

/** 방송 타일 아래 줄: "1080p · 60fps · H.264" */
export function tileMeta(b: Pick<BroadcastView, "height" | "fps" | "codec">): string {
  return `${qualityLabel(b.height, b.fps)} · ${codecLabel(b.codec)}`;
}

/** "지금 방송 중" 옆 방송 수 */
export function broadcastCount(n: number): string {
  return `${n}명`;
}

/** 방송 화면 주소 */
export function watchHref(serverId: string, broadcasterId: string): string {
  return `#/watch/${serverId}/${broadcasterId}`;
}

/**
 * 방송 타일을 눌렀을 때 갈 곳. 내 방송은 시청 화면 대신 방송 화면으로 간다
 * (D-065: 이 PC에서 내 방송 소리를 틀면 루프백이 다시 담아 에코가 쌓인다).
 */
export function tileHref(serverId: string, b: Pick<BroadcastView, "broadcasterId" | "mine">): string {
  return b.mine ? "#/broadcast" : watchHref(serverId, b.broadcasterId);
}

/** 방송 시작 화면 주소(이 서버를 미리 고른다) */
export function publishHref(serverId: string): string {
  return `#/broadcast?server=${serverId}`;
}
