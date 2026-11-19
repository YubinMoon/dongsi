import { listen } from "@tauri-apps/api/event";
import { api } from "$lib/api";
import { errorMessage } from "$lib/errors";
import type { ServerView } from "$lib/types";

/** 저장한 서버 목록. 컴포넌트는 필드를 읽고, 바꿀 때는 `refreshServers()`를 부른다. */
export const serverStore = $state({
  list: [] as ServerView[],
  /** 처음 읽기가 끝났는가(성공이든 실패든) */
  loaded: false,
  error: null as string | null,
});

export async function refreshServers(): Promise<void> {
  try {
    serverStore.list = await api.listServers();
    serverStore.error = null;
  } catch (e) {
    serverStore.error = errorMessage(e);
  } finally {
    serverStore.loaded = true;
  }
}

let listening = false;

/** 앱 시작 때 한 번(레이아웃): 백엔드가 목록을 바꾸면(`servers-changed`, 예: 감지된 서버 저장 D-057) 다시 읽는다. */
export async function initServerEvents(): Promise<void> {
  if (listening) return;
  listening = true;
  try {
    await listen("servers-changed", () => {
      void refreshServers();
    });
  } catch (e) {
    // Tauri 밖(브라우저에서 화면만 볼 때)에서는 이벤트를 들을 수 없다.
    console.warn("servers-changed를 들을 수 없습니다", e);
  }
}

export function serverById(id: string): ServerView | undefined {
  return serverStore.list.find((s) => s.id === id);
}
