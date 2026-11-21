import { listen } from "@tauri-apps/api/event";
import { api } from "$lib/api";
import type { ServerClosedEvent, ThumbnailEvent } from "$lib/types";

/**
 * 방송 목록 썸네일(D-076). 방송 타일(`BroadcastTile`)이 그릴 때 서버 key + 방송자 ID로 그림을 찾는다.
 *
 * - 서버 목록 항목에는 그림 번호만 온다. 그림은 타일이 보일 때 백엔드(`broadcast_thumbnail`)에 묻는다.
 *   백엔드는 번호가 같으면 기억한 그림을 주고, 아니면 서버에 요청한다.
 * - 번호가 바뀌면 백엔드가 `broadcast-thumbnail`을 보낸다. 보이는 타일이면 다시 묻고, 번호가 없어졌으면(방송 끝,
 *   그림 없음) 지운다. 보이지 않는 타일의 그림은 다음에 보일 때 다시 묻는다(그동안은 지난 그림을 둔다).
 * - 서버 연결이 끊기면(`server-closed`) 그 서버의 그림을 모두 지운다.
 * - 홈·서버 화면 파일을 고치지 않으려고 타일이 이미 가진 시청 주소(`#/watch/<서버 key>/<방송자 ID>`)에서 찾는다.
 */

/** 그림 key: `<서버 key>/<방송자 ID>`(홈 `LiveTile.key`와 같은 모양) → `data:image/jpeg;base64,…` */
export const thumbs = $state({ images: {} as Record<string, string> });

/** 그림을 찾는 대상 */
export interface ThumbTarget {
  server: string;
  broadcaster: string;
}

export function thumbKey(server: string, broadcaster: string): string {
  return `${server}/${broadcaster}`;
}

/** 시청 주소(`#/watch/<서버 key>/<방송자 ID>`)에서 대상을 읽는다. 시청 주소가 아니면(내 방송 타일 등) null. */
export function watchTarget(href: string | null | undefined): ThumbTarget | null {
  const m = /^#\/watch\/([^/?#]+)\/([^/?#]+)/.exec(href ?? "");
  if (!m) return null;
  try {
    return { server: decodeURIComponent(m[1]), broadcaster: decodeURIComponent(m[2]) };
  } catch {
    return null;
  }
}

/** 그 타일의 그림(없으면 null). 화면은 이 값을 읽어 다시 그린다. */
export function thumbnailOf(target: ThumbTarget | null): string | null {
  return target ? (thumbs.images[thumbKey(target.server, target.broadcaster)] ?? null) : null;
}

/** 보이는 타일 수(그림 key → 수) */
const refs = new Map<string, number>();
/** 마지막으로 받은 그림 번호 */
const ids = new Map<string, number>();
/** 묻는 중인 그림, 묻는 동안 번호가 또 바뀐 그림 */
const inflight = new Map<string, Promise<void>>();
const dirty = new Set<string>();
let listening: Promise<void> | null = null;

function forget(key: string) {
  delete thumbs.images[key];
  ids.delete(key);
}

async function fetchThumbnail(server: string, broadcaster: string): Promise<void> {
  const key = thumbKey(server, broadcaster);
  try {
    const t = await api.broadcastThumbnail(server, broadcaster);
    if (t) {
      thumbs.images[key] = t.image;
      ids.set(key, t.id);
    } else {
      forget(key);
    }
  } catch {
    // 연결이 끊겼거나 명령이 없는 백엔드: 지난 그림을 그대로 둔다.
  }
}

/** 그림을 다시 묻는다. 묻는 중이면 끝난 뒤 한 번 더 묻는다. */
function refresh(server: string, broadcaster: string): Promise<void> {
  const key = thumbKey(server, broadcaster);
  const running = inflight.get(key);
  if (running) {
    dirty.add(key);
    return running;
  }
  const p = fetchThumbnail(server, broadcaster).finally(() => {
    inflight.delete(key);
    if (dirty.delete(key) && refs.has(key)) void refresh(server, broadcaster);
  });
  inflight.set(key, p);
  return p;
}

/** 백엔드 이벤트 하나(시험에서 바로 부른다). */
export function onThumbnailEvent({ key: server, broadcasterId, id }: ThumbnailEvent) {
  const key = thumbKey(server, broadcasterId);
  if (id === null) {
    forget(key);
    return;
  }
  if (ids.get(key) === id) return;
  if (refs.has(key)) void refresh(server, broadcasterId);
}

/** 서버 연결이 끊겼다: 그 서버의 그림을 모두 지운다. */
export function onServerClosed(server: string) {
  const prefix = `${server}/`;
  for (const key of Object.keys(thumbs.images)) if (key.startsWith(prefix)) forget(key);
  for (const key of [...ids.keys()]) if (key.startsWith(prefix)) ids.delete(key);
}

function ensureListening() {
  listening ??= (async () => {
    try {
      await listen<ThumbnailEvent>("broadcast-thumbnail", (e) => onThumbnailEvent(e.payload));
      await listen<ServerClosedEvent>("server-closed", (e) => onServerClosed(e.payload.key));
    } catch (e) {
      // Tauri 밖(화면만 볼 때)에서는 이벤트를 들을 수 없다.
      console.warn("썸네일 이벤트를 들을 수 없습니다", e);
    }
  })();
}

/**
 * 타일이 보이기 시작할 때 부른다(돌려받은 함수를 사라질 때 부른다). 처음 보이면 그림을 묻는다.
 * 같은 그림을 여러 타일이 보여도(홈과 서버 화면) 한 번만 묻는다.
 */
export function trackThumbnail(target: ThumbTarget): () => void {
  ensureListening();
  const key = thumbKey(target.server, target.broadcaster);
  const n = (refs.get(key) ?? 0) + 1;
  refs.set(key, n);
  if (n === 1) void refresh(target.server, target.broadcaster);
  let done = false;
  return () => {
    if (done) return;
    done = true;
    const left = (refs.get(key) ?? 1) - 1;
    if (left > 0) refs.set(key, left);
    else refs.delete(key);
  };
}

/** 시험용: 상태를 비운다. */
export function resetThumbnails() {
  for (const key of Object.keys(thumbs.images)) delete thumbs.images[key];
  refs.clear();
  ids.clear();
  inflight.clear();
  dirty.clear();
  listening = null;
}
