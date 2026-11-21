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
import { refreshServers, serverById, serverStore } from "./servers.svelte";

/** 홈 서버 카드 안에 보이는 접속 상태(탭도 서버 화면도 없이 카드에만 보인다). */
export type CardState =
  | { kind: "connecting" }
  /** 사용자가 카드·다시 연결을 눌러 시도했다가 실패했다. */
  | { kind: "failed"; message: string }
  /** 자동 연결(D-071)이 실패했다. 카드에 조용히 보이고, 뒤에서 다시 시도한다. */
  | { kind: "offline"; message: string }
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

// ---------- 자동 연결(D-071) ----------
// 저장한 서버에는 앱을 켜면 알아서 연결해 둔다(홈 "방송 중"에 그 서버의 방송을 보이려고).
// 실패하면 카드에 조용히(offline) 남기고 5·10·20·40·60초… 간격으로 다시 시도한다.
// 사용자가 끊은 서버(탭 ×, 서버 메뉴 "연결 끊기", 삭제)는 이번 실행에서 다시 자동 연결하지 않는다.
// 카드를 눌러 직접 연결하면 다시 자동 연결 대상이 된다.

/** 자동 연결을 살피는 간격 */
export const AUTO_TICK_MS = 2000;
/** 자동 연결 실패 뒤 다시 시도하기까지: 5초에서 두 배씩, 최대 60초 */
const RETRY_BASE_MS = 5000;
const RETRY_MAX_MS = 60_000;
/** 연결돼 있던 서버가 끊기면 이만큼 뒤에 다시 시도한다(서버를 다시 켜는 중일 수 있다). */
const RECONNECT_DELAY_MS = 2000;

/** 서버 key → 다음 자동 연결 시각과 잇단 실패 횟수. 없으면 바로 시도한다. */
const retry = new Map<string, { failures: number; nextAt: number }>();
/** 사용자가 끊은 서버(자동 연결하지 않는다). */
const paused = new Set<string>();
/** 지금 사용자가 직접(카드·다시 연결) 시도하는 서버. 실패하면 offline이 아니라 failed로 보인다. */
const loud = new Set<string>();
let autoTimer: ReturnType<typeof setInterval> | undefined;

/** 자동 연결 실패 횟수에 따른 다음 시도까지의 시간 */
export function retryDelay(failures: number): number {
  return Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** Math.max(0, failures - 1));
}

export function isConnected(key: string): boolean {
  return conn.tabs.order.includes(key) && !conn.tabs.closed.includes(key);
}

/** 자동 연결을 다시 시도할 시각을 `at`으로 당긴다(끊긴 서버, 감지로 새로 찾은 서버, 새로 고침). */
function retrySoon(key: string, at: number) {
  const r = retry.get(key);
  retry.set(key, { failures: r?.failures ?? 0, nextAt: Math.min(r?.nextAt ?? at, at) });
}

/** 자동 연결할 서버인가: 연결 안 됨, 시도 중 아님, 사용자가 끊지 않음, 지문 경고 없음(사용자가 확인해야 한다). */
function wantsAuto(key: string): boolean {
  if (isConnected(key) || pending.has(key) || paused.has(key) || disconnecting.has(key)) return false;
  const card = conn.card[key]?.kind;
  return card !== "fingerprint" && card !== "connecting";
}

/**
 * 저장한 서버 중 연결 안 된 것에 자동으로 연결한다(`AUTO_TICK_MS`마다, 시도할 때가 된 서버만).
 * 서버 목록을 아직 못 읽었으면 아무것도 하지 않는다.
 */
export function autoConnectTick(now = Date.now()) {
  if (!serverStore.loaded || serverStore.error !== null) return;
  for (const { id } of serverStore.list) {
    if (!wantsAuto(id)) continue;
    const r = retry.get(id);
    if (r && now < r.nextAt) continue;
    void connect(id, { quiet: true });
  }
}

/** 앱 시작 때 한 번(레이아웃, 메인 창만): 자동 연결을 시작한다. */
export function startAutoConnect() {
  if (autoTimer !== undefined) return;
  autoTimer = setInterval(() => autoConnectTick(), AUTO_TICK_MS);
  autoConnectTick();
}

/** 시험용: 자동 연결을 멈추고 상태를 비운다. */
export function resetAutoConnect() {
  if (autoTimer !== undefined) clearInterval(autoTimer);
  autoTimer = undefined;
  retry.clear();
  paused.clear();
  loud.clear();
}

/**
 * 연결 안 된 서버를 기다리지 않고 바로 다시 시도한다. `keys`가 없으면 저장한 서버 전부.
 * 사용자가 끊은 서버와 지문 경고가 있는 서버는 그대로 둔다.
 */
export function connectSoon(keys?: string[]) {
  const now = Date.now();
  for (const key of keys ?? serverStore.list.map((s) => s.id)) {
    if (wantsAuto(key)) retrySoon(key, now);
  }
  autoConnectTick(now);
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
      // 사용자가 끊은 것이 아니면 곧 다시 자동 연결한다(D-071).
      if (!paused.has(key)) retrySoon(key, Date.now() + RECONNECT_DELAY_MS);
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
  const firstTime = serverById(key)?.fingerprint === null;
  try {
    const live = await api.connectServer(key);
    // 기다리는 동안 사용자가 끊었으면(탭 ×와 겹침) 결과를 버리고 늦게 열린 연결도 닫는다.
    if (paused.has(key) && !loud.has(key)) {
      void api.disconnectServer(key).catch(() => {});
      return null;
    }
    retry.delete(key);
    delete conn.card[key];
    delete conn.closedReason[key];
    setTabs(addConnected(conn.tabs, key));
    conn.live[key] = live;
    // 첫 접속이면 이때 인증서 지문이 저장됐으니 목록을 다시 읽는다.
    if (firstTime) await refreshServers();
    return live;
  } catch (e) {
    const message = errorMessage(e);
    const failures = (retry.get(key)?.failures ?? 0) + 1;
    retry.set(key, { failures, nextAt: Date.now() + retryDelay(failures) });
    if (errorCode(e) === "fingerprintMismatch") {
      const detail = fingerprintMismatch(e);
      conn.card[key] = {
        kind: "fingerprint",
        message,
        stored: detail?.stored ?? serverById(key)?.fingerprint ?? null,
        actual: detail?.actual ?? null,
      };
    } else if (loud.has(key)) {
      conn.card[key] = { kind: "failed", message };
    } else if (conn.card[key]?.kind !== "failed") {
      // 자동 연결 실패. 사용자가 직접 시도해 남은 실패(failed)는 그대로 둔다.
      conn.card[key] = { kind: "offline", message };
    }
    return null;
  }
}

/**
 * 서버에 연결한다. 성공하면 탭을 더하고 서버 상태를 돌려주고, 실패하면 홈 카드에 오류를 남기고 null.
 * 같은 key를 동시에 부르면 하나로 합친다(자동 연결 중에 카드를 누르면 그 시도를 같이 기다린다).
 * - 기본(사용자가 누름): 카드에 "연결하는 중…"을 보이고, 실패는 failed로 남긴다. 사용자가 끊었던 서버도 다시 자동 연결 대상이 된다.
 * - `quiet`(자동 연결): 카드에 "연결하는 중…"을 보이지 않고, 실패는 offline으로 남긴다.
 */
export function connect(key: string, opts: { quiet?: boolean } = {}): Promise<ServerLive | null> {
  if (!opts.quiet) {
    loud.add(key);
    paused.delete(key);
    conn.card[key] = { kind: "connecting" };
  }
  const running = pending.get(key);
  if (running) return running;
  const p = doConnect(key).finally(() => {
    pending.delete(key);
    loud.delete(key);
    if (conn.card[key]?.kind === "connecting") delete conn.card[key];
  });
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
  // 사용자가 끊은 서버는 다시 자동 연결하지 않는다(카드를 누르면 다시 연결된다, D-071).
  paused.add(key);
  retry.delete(key);
  if (conn.card[key]?.kind === "offline") delete conn.card[key];
  try {
    if (!opts.localOnly) {
      try {
        await api.disconnectServer(key);
      } catch (e) {
        // T2 이전 백엔드에는 명령이 없다. 그때는 탭만 지운다.
        if (!isMissingCommand(e)) {
          // 끊지 못했으니(방송 중인 서버) 자동 연결 대상으로 되돌린다.
          paused.delete(key);
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
