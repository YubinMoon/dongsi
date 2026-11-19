import { listen } from "@tauri-apps/api/event";
import { api } from "$lib/api";
import { errorCode, errorMessage, fingerprintMismatch, isMissingCommand } from "$lib/errors";
import {
  addConnected,
  mergeBackend,
  onServerClosed,
  onViewingChanged,
  removeKey,
  type TabState,
} from "$lib/tabs";
import { toast } from "$lib/toast.svelte";
import type { FingerprintView, KeyEvent, ServerClosedEvent, ServerLive } from "$lib/types";
import { refreshServers, serverById } from "./servers.svelte";

/** 홈 서버 카드 안에 보이는 접속 상태(탭도 서버 화면도 없이 카드에만 보인다). */
export type CardState =
  | { kind: "connecting" }
  | { kind: "failed"; message: string }
  | {
      kind: "fingerprint";
      message: string;
      stored: FingerprintView | null;
      actual: FingerprintView | null;
    };

export const conn = $state({
  /** 위쪽 바 탭 상태. 그릴 때는 `computeTabs`를 쓴다. */
  tabs: { order: [], closed: [] } as TabState,
  /** 지금 서버 화면으로 보고 있는 서버 key */
  viewing: null as string | null,
  /** 도중에 끊긴 서버의 이유(`server-closed`의 reason) */
  closedReason: {} as Record<string, string>,
  /** 서버 key → 홈 카드의 접속 상태 */
  card: {} as Record<string, CardState>,
  /** 연결된 서버 key → 마지막으로 읽은 서버 상태(방송 목록 등) */
  live: {} as Record<string, ServerLive>,
});

let inited = false;
/** 탭 상태가 바뀔 때마다 올린다. 늦게 도착한 `connected_servers` 응답이 더 새로운 상태를 덮지 않게 한다. */
let epoch = 0;
const pending = new Map<string, Promise<ServerLive | null>>();
/** 사용자가 끊는 중인 서버. 그동안 오는 `server-closed`("연결을 끊었습니다")는 끊김 화면을 띄우지 않게 무시한다. */
const disconnecting = new Set<string>();

export function isConnected(key: string): boolean {
  return conn.tabs.order.includes(key) && !conn.tabs.closed.includes(key);
}

function setTabs(next: TabState) {
  epoch++;
  conn.tabs = next;
  // 탭에서 사라진 서버의 부속 상태도 지운다.
  for (const key of Object.keys(conn.live)) {
    if (!next.order.includes(key)) delete conn.live[key];
  }
  for (const key of Object.keys(conn.closedReason)) {
    if (!next.order.includes(key)) delete conn.closedReason[key];
  }
}

export function dismissCard(key: string) {
  delete conn.card[key];
}

/** 서버 화면이 마운트되면 key, 해제되면 null. 보던 서버가 아닌 끊긴 탭은 이때 사라진다. */
export function setViewing(key: string | null) {
  conn.viewing = key;
  setTabs(onViewingChanged(conn.tabs, key));
}

/** `connected_servers`로 탭을 맞춘다. 화면을 옮길 때마다 부른다. */
export async function syncConnected(): Promise<void> {
  const before = epoch;
  try {
    const keys = await api.connectedServers();
    // 읽는 사이에 연결·끊기가 있었으면 이 응답은 낡았다. 다음 이동 때 다시 맞춘다.
    if (before !== epoch) return;
    setTabs(mergeBackend(conn.tabs, keys, conn.viewing));
  } catch (e) {
    if (!isMissingCommand(e)) console.warn("connected_servers 실패", e);
  }
}

async function refreshLive(key: string): Promise<void> {
  if (!isConnected(key)) return;
  try {
    const live = await api.connectServer(key);
    if (isConnected(key)) conn.live[key] = live;
  } catch {
    // 연결이 끊긴 것이면 곧 server-closed가 온다.
  }
}

/** 앱 시작 때 한 번(레이아웃): 이벤트를 듣고 백엔드의 연결 목록과 맞춘다. */
export async function initConnections(): Promise<void> {
  if (inited) return;
  inited = true;
  try {
    await listen<ServerClosedEvent>("server-closed", (e) => {
      const { key, reason } = e.payload;
      // 직접 끊는 중이거나 이미 탭에서 지운 서버의 알림은 무시한다.
      if (disconnecting.has(key) || !conn.tabs.order.includes(key)) return;
      setTabs(onServerClosed(conn.tabs, key, conn.viewing));
      if (conn.tabs.order.includes(key)) conn.closedReason[key] = reason;
    });
    await listen<KeyEvent>("server-changed", (e) => {
      void refreshLive(e.payload.key);
    });
  } catch (e) {
    // Tauri 밖(브라우저에서 화면만 볼 때)에서는 이벤트를 들을 수 없다.
    console.warn("서버 이벤트를 들을 수 없습니다", e);
  }
  await syncConnected();
}

async function doConnect(key: string): Promise<ServerLive | null> {
  conn.card[key] = { kind: "connecting" };
  const firstTime = serverById(key)?.fingerprint === null;
  try {
    const live = await api.connectServer(key);
    delete conn.card[key];
    delete conn.closedReason[key];
    setTabs(addConnected(conn.tabs, key));
    conn.live[key] = live;
    // 첫 접속이면 이때 인증서 지문이 저장됐으니 목록을 다시 읽는다.
    if (firstTime) await refreshServers();
    return live;
  } catch (e) {
    const message = errorMessage(e);
    if (errorCode(e) === "fingerprintMismatch") {
      const detail = fingerprintMismatch(e);
      conn.card[key] = {
        kind: "fingerprint",
        message,
        stored: detail?.stored ?? serverById(key)?.fingerprint ?? null,
        actual: detail?.actual ?? null,
      };
    } else {
      conn.card[key] = { kind: "failed", message };
    }
    return null;
  }
}

/**
 * 서버에 연결한다. 성공하면 탭을 더하고 서버 상태를 돌려주고, 실패하면 홈 카드에 오류를 남기고 null.
 * 같은 key를 동시에 부르면 하나로 합친다.
 */
export function connect(key: string): Promise<ServerLive | null> {
  const running = pending.get(key);
  if (running) return running;
  const p = doConnect(key).finally(() => pending.delete(key));
  pending.set(key, p);
  return p;
}

/**
 * 연결을 끊고 탭을 없앤다. 그 서버 화면을 보고 있었으면 홈으로 간다.
 * 백엔드가 거절하면(방송 중인 서버: 코드 `broadcasting`) 알림을 띄우고 탭을 그대로 두며 false.
 * `localOnly`: 백엔드가 이미 연결을 닫은 경우(서버 삭제) 탭만 지운다.
 */
export async function disconnect(key: string, opts: { localOnly?: boolean } = {}): Promise<boolean> {
  disconnecting.add(key);
  try {
    if (!opts.localOnly) {
      try {
        await api.disconnectServer(key);
      } catch (e) {
        // T2 이전 백엔드에는 명령이 없다. 그때는 탭만 지운다.
        if (!isMissingCommand(e)) {
          toast(errorMessage(e), "error");
          return false;
        }
      }
    }
    const wasViewing = conn.viewing === key;
    setTabs(removeKey(conn.tabs, key));
    if (wasViewing) location.hash = "#/";
    return true;
  } finally {
    disconnecting.delete(key);
  }
}
