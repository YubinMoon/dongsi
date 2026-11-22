import { codecLabel, qualityLabel } from "./format";
import type { IconName } from "./components/icons";
import type { ServerStatus, ServerStatusKind } from "./tabs";
import type { BroadcastView } from "./types";

/**
 * 서버 화면이 보여 줄 것. 머리(이름·주소)는 서버가 있으면 늘 보이고, 아래 자리가 바뀐다.
 * 서버 메뉴(연결 끊기·다시 연결·이름 변경·삭제)는 서버 화면에 두지 않고 위쪽 탭에만 둔다(D-079).
 * - missing: 서버 목록에 없다(삭제됨)
 * - loading: 서버 목록을 아직 못 읽었다
 * - live: 연결돼 있다(방송 목록)
 * - connecting: 연결하는 중이거나 아직 시도 전(들어오면 바로 시도한다)
 * - down: 연결 안 됨(끊김·자동 연결 실패·실패·지문 경고·사용자가 끊음). 이유와 "다시 연결"을 보인다(D-073).
 */
export type ServerScreen = "missing" | "loading" | "live" | "connecting" | "down";

export function serverScreen(input: { listLoaded: boolean; exists: boolean; status: ServerStatusKind }): ServerScreen {
  if (!input.exists) return input.listLoaded ? "missing" : "loading";
  if (input.status === "connected") return "live";
  if (input.status === "connecting" || input.status === "idle") return "connecting";
  return "down";
}

/** 연결 안 된 서버 화면의 안내(방송 목록 자리). 지문 경고는 따로 그린다(저장·실제 지문 비교). */
export interface DownView {
  tone: "neutral" | "danger";
  icon: IconName;
  title: string;
  /** 설명 줄. 없으면 null. */
  detail: string | null;
}

export function downView(status: ServerStatus): DownView | null {
  switch (status.kind) {
    case "closed":
      // D-055: 한 줄 + 서버가 알린 이유(기본 문구면 없음) + 다시 연결
      return { tone: "neutral", icon: "alert", title: "서버와 연결이 끊어졌습니다.", detail: status.reason };
    case "offline":
      return { tone: "neutral", icon: "alert", title: "서버에 연결하지 못했습니다.", detail: status.title };
    case "failed":
      return { tone: "danger", icon: "alert", title: "서버에 연결하지 못했습니다.", detail: status.title };
    case "paused":
      return {
        tone: "neutral",
        icon: "server",
        title: "연결을 끊었습니다.",
        detail: "이번 실행 동안 자동으로 연결하지 않습니다.",
      };
    default:
      return null;
  }
}

/** 지문 짧은 표시의 앞 `groups`묶음("3A:9F:12:C4"). 저장·실제 지문을 한 줄에 나란히 두려고 줄인다. */
export function fingerprintPrefix(short: string, groups = 4): string {
  return short.split(":").slice(0, groups).join(":");
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
