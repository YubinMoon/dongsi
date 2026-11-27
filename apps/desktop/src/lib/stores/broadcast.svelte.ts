import { listen } from "@tauri-apps/api/event";
import { api } from "$lib/api";
import { playViewerChime } from "$lib/chime";
import type { BroadcastStatus, ViewerCountEvent } from "$lib/types";
import { initialChimeState, observeViewers, settleViewers } from "$lib/viewerChime";
import { isPopoutWindow } from "$lib/watch";

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

// 시청자 입장·퇴장 알림음의 판정 상태. 방송 하나 동안만 유지한다.
let chimeBroadcast: string | null = null;
let chime = initialChimeState();
let chimeTimer: ReturnType<typeof setTimeout> | null = null;

/** 방송이 끝나거나 바뀌면 판정을 처음부터 한다. 새 방송의 기준값은 처음 받은 상태의 시청자 수다. */
function trackChimeBroadcast(status: BroadcastStatus | null) {
  const id = status ? `${status.key}/${status.startedAt}` : null;
  if (id === chimeBroadcast) return;
  chimeBroadcast = id;
  if (chimeTimer !== null) clearTimeout(chimeTimer);
  chimeTimer = null;
  chime = initialChimeState();
  // 방송 시작 직후의 `viewer-count`는 상태보다 먼저 와서 버려질 수 있어, 기준값을 여기서 잡는다.
  if (status) chime = observeViewers(chime, status.viewers, Date.now());
}

function onViewerCount({ key, count }: ViewerCountEvent) {
  const status = onAir.status;
  if (!status || status.key !== key) return;
  // 방송 화면의 숫자는 다음 폴링을 기다리지 않고 바로 고친다.
  status.viewers = count;
  const now = Date.now();
  chime = observeViewers(chime, count, now);
  if (chime.windowEnd === null || chimeTimer !== null) return;
  chimeTimer = setTimeout(() => {
    chimeTimer = null;
    const settled = settleViewers(chime);
    chime = settled.state;
    if (settled.chime) playViewerChime(settled.chime);
  }, chime.windowEnd - now);
}

export function setBroadcastStatus(status: BroadcastStatus | null) {
  trackChimeBroadcast(status);
  onAir.status = status;
  syncTimer();
}

/** 백엔드의 `broadcast_status`를 다시 읽는다. */
export async function refreshBroadcastStatus(): Promise<void> {
  try {
    setBroadcastStatus(await api.broadcastStatus());
  } catch (e) {
    console.warn("broadcast_status 실패", e);
  }
}

/** 앱 시작 때 한 번: 상태를 읽고 `broadcast-stopped`·`viewer-count` 이벤트를 듣는다. */
export async function initBroadcastState(): Promise<void> {
  if (inited) return;
  inited = true;
  try {
    await listen("broadcast-stopped", () => setBroadcastStatus(null));
    // 이벤트는 모든 창에 간다. 알림음이 두 번 나지 않게 분리 시청 창은 듣지 않는다.
    if (!isPopoutWindow()) {
      await listen<ViewerCountEvent>("viewer-count", (e) => onViewerCount(e.payload));
    }
  } catch (e) {
    // Tauri 밖(브라우저에서 화면만 볼 때)에서는 이벤트를 들을 수 없다.
    console.warn("방송 이벤트를 들을 수 없습니다", e);
  }
  await refreshBroadcastStatus();
}
