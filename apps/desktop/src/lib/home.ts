import { codecLabel, qualityLabel, viaLabel } from "./format";
import type { CardState } from "./stores/connections.svelte";
import type { FingerprintView, LocalServerView, ServerView } from "./types";

/** 홈 서버 카드의 상태 줄 종류. 접속 시도 상태(connecting·failed·fingerprint)가 저장된 상태보다 우선한다. */
export type CardStatus =
  | { kind: "connecting" }
  | { kind: "failed"; message: string }
  | {
      kind: "fingerprint";
      message: string;
      /** 저장한 지문과 실제 지문을 둘 다 알 때만 있다(없으면 message만 보인다). */
      compare: { stored: FingerprintView; actual: FingerprintView } | null;
    }
  | { kind: "connected"; fingerprinted: boolean; savedPasswords: number }
  | { kind: "known"; savedPasswords: number }
  | { kind: "firstTime" };

/** 카드 상태 줄 종류를 고른다. `connected`는 `isConnected(id)`, `cardState`는 `conn.card[id]`. */
export function cardStatus(
  server: Pick<ServerView, "fingerprint" | "savedPasswords">,
  connected: boolean,
  cardState: CardState | undefined,
): CardStatus {
  if (cardState) {
    switch (cardState.kind) {
      case "connecting":
        return { kind: "connecting" };
      case "failed":
        return { kind: "failed", message: cardState.message };
      case "fingerprint": {
        const { stored, actual } = cardState;
        return {
          kind: "fingerprint",
          message: cardState.message,
          compare: stored && actual ? { stored, actual } : null,
        };
      }
    }
  }
  if (connected) {
    return {
      kind: "connected",
      fingerprinted: server.fingerprint !== null,
      savedPasswords: server.savedPasswords,
    };
  }
  if (server.fingerprint === null) return { kind: "firstTime" };
  return { kind: "known", savedPasswords: server.savedPasswords };
}

/** 카드를 눌렀을 때: 연결돼 있으면 열기, 접속 중이면 무시, 아니면 연결. */
export function cardClickAction(connected: boolean, cardState: CardState | undefined): "open" | "connect" | "none" {
  if (cardState?.kind === "connecting") return "none";
  return connected ? "open" : "connect";
}

/** 지금 홈 화면(`#/`)에 있는가. 연결이 끝났을 때 사용자가 이미 다른 곳으로 갔으면 끌어오지 않기 위해 본다. */
export function isHomeHash(hash: string): boolean {
  return hash === "" || hash === "#" || hash === "#/" || hash.startsWith("#/?");
}

/** 같은 네트워크 격자의 타일 하나(`BroadcastTile` 입력). */
export interface LocalTile {
  key: string;
  seed: string;
  name: string;
  via: string;
  locked: boolean;
  meta: string;
  href: string;
}

/** 로컬 감지 결과를 방송 타일 목록으로 펼친다. null이면 아직 못 찾은 것이다. */
export function localTiles(servers: LocalServerView[] | null): LocalTile[] {
  if (!servers) return [];
  return servers.flatMap((server) =>
    server.broadcasts.map((b) => ({
      key: `${server.key}/${b.broadcasterId}`,
      seed: b.broadcasterId,
      name: b.broadcasterName,
      via: viaLabel(server.via),
      locked: b.locked,
      meta: `${server.name} · ${qualityLabel(b.height, b.fps)} · ${codecLabel(b.codec)}`,
      href: `#/watch/${server.key}/${b.broadcasterId}`,
    })),
  );
}

/** 지문 짧은 표시의 앞 `groups`묶음("3A:9F:12:C4"). 카드 한 줄에 저장·실제 지문을 나란히 두려고 줄인다(시안 b-home-fingerprint). */
export function fingerprintPrefix(short: string, groups = 4): string {
  return short.split(":").slice(0, groups).join(":");
}

/** "3개" */
export function countLabel(n: number): string {
  return `${n}개`;
}

/** 받침 없음 또는 ㄹ 받침이면 "로", 그 밖의 받침이면 "으로". 한글이 아니면 "(으)로". */
export function euro(name: string): string {
  const last = name.trim().slice(-1);
  if (!last) return "(으)로";
  const code = last.charCodeAt(0);
  if (code < 0xac00 || code > 0xd7a3) return "(으)로";
  const jong = (code - 0xac00) % 28;
  return jong === 0 || jong === 8 ? "로" : "으로";
}
