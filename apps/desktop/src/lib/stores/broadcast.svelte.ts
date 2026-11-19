import { listen } from "@tauri-apps/api/event";
import { api } from "$lib/api";
import { isMissingCommand } from "$lib/errors";
import type { BroadcastStatus } from "$lib/types";

/** 내 방송 상태. 위쪽 바의 "● 방송 중 12:34"가 읽는다. */
export const onAir = $state({
  status: null as BroadcastStatus | null,
  /** 방송 중에만 1초마다 갱신되는 현재 시각(ms) */
  now: Date.now(),
});

let timer: ReturnType<typeof setInterval> | null = null;
let inited = false;

function syncTimer() {
  if (onAir.status && timer === null) {
    onAir.now = Date.now();
    timer = setInterval(() => {
      onAir.now = Date.now();
    }, 1000);
  } else if (!onAir.status && timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}

export function setBroadcastStatus(status: BroadcastStatus | null) {
  onAir.status = status;
  syncTimer();
}

/** 백엔드의 `broadcast_status`를 다시 읽는다. */
export async function refreshBroadcastStatus(): Promise<void> {
  try {
    setBroadcastStatus(await api.broadcastStatus());
  } catch (e) {
    if (!isMissingCommand(e)) console.warn("broadcast_status 실패", e);
  }
}

/** 앱 시작 때 한 번: 상태를 읽고 `broadcast-stopped` 이벤트를 듣는다. */
export async function initBroadcastState(): Promise<void> {
  if (inited) return;
  inited = true;
  try {
    await listen("broadcast-stopped", () => setBroadcastStatus(null));
  } catch (e) {
    // Tauri 밖(브라우저에서 화면만 볼 때)에서는 이벤트를 들을 수 없다.
    console.warn("broadcast-stopped 이벤트를 들을 수 없습니다", e);
  }
  await refreshBroadcastStatus();
}
