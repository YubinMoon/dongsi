import { listen } from "@tauri-apps/api/event";
import { api } from "$lib/api";
import { errorCode, errorMessage, fingerprintMismatch } from "$lib/errors";
import { addConnected, mergeBackend, removeKey, serverStatus, type AttemptState, type ServerStatus } from "$lib/tabs";
import { toast } from "$lib/toast.svelte";
import type { KeyEvent, ServerClosedEvent, ServerLive } from "$lib/types";
import { refreshServers, serverById, serverStore } from "./servers.svelte";

export type { AttemptState } from "$lib/tabs";

/**
 * 서버 연결 상태. 위쪽 바 탭(`computeTabs`)과 서버 화면(`statusOf`)이 읽는다.
 * 저장한 서버는 연결돼 있든 아니든 모두 탭으로 보인다(D-073).
 */
export const conn = $state({
  /** 연결된 서버 key(연결한 순서). 백엔드 `connected_servers`와 맞춘다. */
  connected: [] as string[],
  /**
   * 서버 key → 마지막 접속 시도 결과(연결 안 된 서버만). 탭·서버 화면·시청 화면이 보인다.
   * 이름은 홈 서버 카드 시절 그대로다(시청 화면이 같이 쓴다).
   */
  card: {} as Record<string, AttemptState>,
  /** 지금 접속을 시도하는 서버(자동 연결 포함) */
  trying: {} as Record<string, true>,
  /** 사용자가 연결을 끊은 서버. 이번 실행 동안 자동 연결하지 않는다(D-071). */
  paused: {} as Record<string, true>,
  /** 연결돼 있다가 끊긴 서버의 이유(`server-closed`의 reason). 다시 연결되면 지운다. */
  closedReason: {} as Record<string, string>,
  /** 연결된 서버 key → 마지막으로 읽은 서버 상태(방송 목록 등) */
  live: {} as Record<string, ServerLive>,
});

let inited = false;
/** 연결 목록이 바뀔 때마다 올린다. 늦게 도착한 `connected_servers` 응답이 더 새로운 상태를 덮지 않게 한다. */
let epoch = 0;
const pending = new Map<string, Promise<ServerLive | null>>();
/** 사용자가 끊는 중인 서버. 그동안 오는 `server-closed`("연결을 끊었습니다")는 끊김으로 보이지 않게 무시한다. */
const disconnecting = new Set<string>();

// ---------- 자동 연결(D-071) ----------
// 저장한 서버에는 앱을 켜면 알아서 연결해 둔다(홈 "방송 중"에 그 서버의 방송을 보이려고).
// 실패하면 탭에 조용히(offline, 회색 점) 남기고 5·10·20·40·60초… 간격으로 다시 시도한다.
// 사용자가 끊은 서버(탭 메뉴·서버 화면 메뉴 "연결 끊기", 삭제)는 이번 실행에서 다시 자동 연결하지 않는다.
// 사용자가 "다시 연결"로 직접 연결하면 다시 자동 연결 대상이 된다.

/** 자동 연결을 살피는 간격 */
export const AUTO_TICK_MS = 2000;
/** 자동 연결 실패 뒤 다시 시도하기까지: 5초에서 두 배씩, 최대 60초 */
const RETRY_BASE_MS = 5000;
const RETRY_MAX_MS = 60_000;
/** 연결돼 있던 서버가 끊기면 이만큼 뒤에 다시 시도한다(서버를 다시 켜는 중일 수 있다). */
const RECONNECT_DELAY_MS = 2000;

/** 서버 key → 다음 자동 연결 시각과 잇단 실패 횟수. 없으면 바로 시도한다. */
const retry = new Map<string, { failures: number; nextAt: number }>();
/** 지금 사용자가 직접 시도하는 서버. 실패하면 offline이 아니라 failed로 보인다. */
const loud = new Set<string>();
let autoTimer: ReturnType<typeof setInterval> | undefined;

/** 자동 연결 실패 횟수에 따른 다음 시도까지의 시간 */
export function retryDelay(failures: number): number {
  return Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** Math.max(0, failures - 1));
}

export function isConnected(key: string): boolean {
  return conn.connected.includes(key);
}

/** 서버 하나의 상태(탭·서버 화면) */
export function statusOf(key: string): ServerStatus {
  return serverStatus(key, conn);
}

/** 자동 연결을 다시 시도할 시각을 `at`으로 당긴다(끊긴 서버, 감지로 새로 찾은 서버, 새로 고침). */
function retrySoon(key: string, at: number) {
  const r = retry.get(key);
  retry.set(key, { failures: r?.failures ?? 0, nextAt: Math.min(r?.nextAt ?? at, at) });
}

/** 자동 연결할 서버인가: 연결 안 됨, 시도 중 아님, 사용자가 끊지 않음, 지문 경고 없음(사용자가 확인해야 한다). */
function wantsAuto(key: string): boolean {
  if (isConnected(key) || pending.has(key) || conn.paused[key] || disconnecting.has(key)) return false;
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
  loud.clear();
  for (const key of Object.keys(conn.paused)) delete conn.paused[key];
}

/**
 * 연결 안 된 서버를 기다리지 않고 바로 다시 시도한다(조용히). `keys`가 없으면 저장한 서버 전부.
 * 사용자가 끊은 서버와 지문 경고가 있는 서버는 그대로 둔다. 서버 화면에 들어올 때도 부른다.
 */
export function connectSoon(keys?: string[]) {
  const now = Date.now();
  for (const key of keys ?? serverStore.list.map((s) => s.id)) {
    if (wantsAuto(key)) retrySoon(key, now);
  }
  autoConnectTick(now);
}

function setConnected(next: string[]) {
  epoch++;
  conn.connected = next;
  // 연결이 없어진 서버의 방송 목록은 지운다.
  for (const key of Object.keys(conn.live)) {
    if (!next.includes(key)) delete conn.live[key];
  }
}

/** 접속 시도 결과를 지운다(저장하지 않은 서버의 시청이 실패했을 때 시청 화면이 부른다). */
export function dismissCard(key: string) {
  delete conn.card[key];
}

/** `connected_servers`로 연결 목록을 맞춘다. 화면을 옮길 때마다 부른다. */
export async function syncConnected(): Promise<void> {
  const before = epoch;
  try {
    const keys = await api.connectedServers();
    // 읽는 사이에 연결·끊기가 있었으면 이 응답은 낡았다. 다음 이동 때 다시 맞춘다.
    if (before !== epoch) return;
    // 백엔드에서 사라진 연결(끊김 알림을 놓친 경우)은 곧 다시 자동 연결한다.
    for (const key of conn.connected) {
      if (!keys.includes(key) && !conn.paused[key]) retrySoon(key, Date.now() + RECONNECT_DELAY_MS);
    }
    setConnected(mergeBackend(conn.connected, keys));
    // 이쪽이 모르던 연결(앱 새로 고침 뒤 등)은 방송 목록을 읽어 와야 홈 "방송 중"에 보인다.
    for (const key of conn.connected) {
      if (!conn.live[key]) void refreshLive(key);
    }
  } catch (e) {
    console.warn("connected_servers 실패", e);
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
      // 직접 끊는 중이거나 이미 연결 목록에 없는 서버의 알림은 무시한다.
      if (disconnecting.has(key) || !isConnected(key)) return;
      // 사용자가 끊은 것이 아니면 곧 다시 자동 연결한다(D-071).
      if (!conn.paused[key]) retrySoon(key, Date.now() + RECONNECT_DELAY_MS);
      setConnected(removeKey(conn.connected, key));
      conn.closedReason[key] = reason;
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
    // 기다리는 동안 사용자가 끊었으면(연결 끊기·삭제와 겹침) 결과를 버리고 늦게 열린 연결도 닫는다.
    if (conn.paused[key] && !loud.has(key)) {
      void api.disconnectServer(key).catch(() => {});
      return null;
    }
    retry.delete(key);
    delete conn.card[key];
    delete conn.closedReason[key];
    setConnected(addConnected(conn.connected, key));
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
 * 서버에 연결한다. 성공하면 연결 목록에 더하고 서버 상태를 돌려주고, 실패하면 시도 결과(`conn.card`)를 남기고 null.
 * 같은 key를 동시에 부르면 하나로 합친다(자동 연결 중에 "다시 연결"을 누르면 그 시도를 같이 기다린다).
 * - 기본(사용자가 누름): 탭·서버 화면에 "연결하는 중"을 보이고, 실패는 failed(빨강)로 남긴다.
 *   사용자가 끊었던 서버도 다시 자동 연결 대상이 된다.
 * - `quiet`(자동 연결): 실패는 offline(회색)으로 남긴다. 이미 실패가 보이고 있으면 표시를 바꾸지 않는다.
 */
export function connect(key: string, opts: { quiet?: boolean } = {}): Promise<ServerLive | null> {
  if (!opts.quiet) {
    loud.add(key);
    delete conn.paused[key];
    conn.card[key] = { kind: "connecting" };
  }
  const running = pending.get(key);
  if (running) return running;
  conn.trying[key] = true;
  const p = doConnect(key).finally(() => {
    pending.delete(key);
    loud.delete(key);
    delete conn.trying[key];
    if (conn.card[key]?.kind === "connecting") delete conn.card[key];
  });
  pending.set(key, p);
  return p;
}

/**
 * 연결을 끊는다. 탭은 남고(저장한 서버, D-073) "연결 끊음"으로 보이며, 이번 실행 동안 자동 연결하지 않는다.
 * 백엔드가 거절하면(방송 중인 서버: 코드 `broadcasting`) 알림을 띄우고 연결을 그대로 두며 false.
 * `localOnly`: 백엔드가 이미 연결을 닫은 경우(서버 삭제) 이쪽 상태만 지운다.
 */
export async function disconnect(key: string, opts: { localOnly?: boolean } = {}): Promise<boolean> {
  disconnecting.add(key);
  // 사용자가 끊은 서버는 다시 자동 연결하지 않는다("다시 연결"을 누르면 다시 연결된다, D-071).
  conn.paused[key] = true;
  retry.delete(key);
  try {
    if (!opts.localOnly) {
      try {
        await api.disconnectServer(key);
      } catch (e) {
        // 끊지 못했으니(방송 중인 서버) 자동 연결 대상으로 되돌린다.
        delete conn.paused[key];
        toast(errorMessage(e), "error");
        return false;
      }
    }
    delete conn.card[key];
    delete conn.closedReason[key];
    setConnected(removeKey(conn.connected, key));
    return true;
  } finally {
    disconnecting.delete(key);
  }
}
