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

export function serverById(id: string): ServerView | undefined {
  return serverStore.list.find((s) => s.id === id);
}
