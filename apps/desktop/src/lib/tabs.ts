import type { ServerView } from "./types";

/**
 * 위쪽 바의 서버 탭 상태(순수 함수로만 바꾼다. 모든 함수는 새 객체를 돌려준다).
 * - order: 탭 순서인 연결된 서버 key
 * - closed: 연결은 끊겼지만 그 서버 화면을 보고 있어서 남겨 둔 key(order에도 들어 있다)
 */
export interface TabState {
  order: string[];
  closed: string[];
}

export interface TabView {
  id: string;
  name: string;
  closed: boolean;
  active: boolean;
}

export const EMPTY_TABS: TabState = { order: [], closed: [] };

function copy(s: TabState): TabState {
  return { order: [...s.order], closed: [...s.closed] };
}

/** 연결에 성공한 서버를 맨 끝에 더한다. 끊긴 채 남아 있던 탭이면 그 자리에서 되살린다. */
export function addConnected(s: TabState, key: string): TabState {
  const next = copy(s);
  next.closed = next.closed.filter((k) => k !== key);
  if (!next.order.includes(key)) next.order.push(key);
  return next;
}

/** 탭을 없앤다(연결을 직접 끊었거나 서버를 삭제했을 때). */
export function removeKey(s: TabState, key: string): TabState {
  return {
    order: s.order.filter((k) => k !== key),
    closed: s.closed.filter((k) => k !== key),
  };
}

/** 서버가 닫혔다. 그 화면을 보고 있으면 끊긴 탭으로 남기고, 아니면 지운다. 탭에 없던 키면 그대로. */
export function onServerClosed(s: TabState, key: string, viewing: string | null): TabState {
  if (!s.order.includes(key)) return copy(s);
  if (viewing !== key) return removeKey(s, key);
  const next = copy(s);
  if (!next.closed.includes(key)) next.closed.push(key);
  return next;
}

/** 보는 화면이 바뀌었다. 끊긴 채 남은 탭 중 지금 보는 서버가 아닌 것은 지운다. */
export function onViewingChanged(s: TabState, viewing: string | null): TabState {
  const stale = s.closed.filter((k) => k !== viewing);
  if (stale.length === 0) return copy(s);
  return {
    order: s.order.filter((k) => !stale.includes(k)),
    closed: s.closed.filter((k) => !stale.includes(k)),
  };
}

/**
 * 백엔드의 연결 목록(`connected_servers`)에 맞춘다.
 * 백엔드에만 있는 키는 끝에 더하고, 탭에는 있는데(끊긴 탭이 아님) 백엔드에 없는 키는 `onServerClosed`처럼 처리한다.
 */
export function mergeBackend(s: TabState, backendKeys: string[], viewing: string | null): TabState {
  let next = copy(s);
  for (const key of backendKeys) {
    if (!next.order.includes(key)) next.order.push(key);
  }
  for (const key of s.order) {
    if (s.closed.includes(key) || backendKeys.includes(key)) continue;
    next = onServerClosed(next, key, viewing);
  }
  return next;
}

/** 그릴 탭: 저장한 서버 ID인 키만, order 순서대로. */
export function computeTabs(s: TabState, servers: ServerView[], viewing: string | null): TabView[] {
  const tabs: TabView[] = [];
  for (const key of s.order) {
    const server = servers.find((v) => v.id === key);
    if (!server) continue;
    tabs.push({ id: key, name: server.name, closed: s.closed.includes(key), active: key === viewing });
  }
  return tabs;
}
