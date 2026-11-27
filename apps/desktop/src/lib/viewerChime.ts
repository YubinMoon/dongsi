/**
 * 시청자 입장·퇴장 알림음의 판정과 묶기. 순수 함수만 둔다(타이머와 소리는 바깥).
 *
 * - 방송마다 첫 값은 기준값만 잡는다.
 * - 값이 처음 바뀌면 고정 창(`CHIME_WINDOW_MS`)을 열고, 창 안의 변화는 창을 늘리지 않는다.
 * - 창이 끝나면 기준값에서 마지막 값까지의 순증감으로 한 번만 정한다. 시청자의 재접속(-1, +1)은 여기서 사라진다.
 */

export type ViewerChime = "join" | "leave";

export const CHIME_WINDOW_MS = 1500;

export interface ChimeState {
  /** 기준값. 방송의 첫 값 전에는 null. */
  base: number | null;
  /** 마지막으로 받은 값 */
  last: number | null;
  /** 열린 묶기 창이 끝나는 시각(ms). 창이 없으면 null. */
  windowEnd: number | null;
}

/** 방송을 시작하기 전, 방송이 끝나거나 바뀐 뒤의 상태 */
export function initialChimeState(): ChimeState {
  return { base: null, last: null, windowEnd: null };
}

/** 시청자 수를 받는다. 창을 새로 열었으면 `windowEnd`가 생긴다: 바깥이 그때 `settleViewers`를 부른다. */
export function observeViewers(state: ChimeState, count: number, now: number): ChimeState {
  if (state.base === null) return { base: count, last: count, windowEnd: null };
  if (count === state.last) return state;
  return { base: state.base, last: count, windowEnd: state.windowEnd ?? now + CHIME_WINDOW_MS };
}

/** 창이 끝났을 때: 낼 소리(순증감이 없으면 null)와 새 상태. 마지막 값이 새 기준값이 된다. */
export function settleViewers(state: ChimeState): { state: ChimeState; chime: ViewerChime | null } {
  const { base, last } = state;
  const next: ChimeState = { base: last, last, windowEnd: null };
  if (base === null || last === null || last === base) return { state: next, chime: null };
  return { state: next, chime: last > base ? "join" : "leave" };
}
