import { describe, expect, it } from "vitest";
import {
  CHIME_WINDOW_MS,
  initialChimeState,
  observeViewers,
  settleViewers,
  type ChimeState,
} from "./viewerChime";

/** 기준값 `base`인 상태 */
const based = (base: number) => observeViewers(initialChimeState(), base, 0);

/** 값을 100ms 간격으로 차례로 받는다. */
function feed(state: ChimeState, counts: number[], start = 1000): ChimeState {
  return counts.reduce((s, count, i) => observeViewers(s, count, start + i * 100), state);
}

describe("observeViewers·settleViewers", () => {
  it("첫 값은 기준값만 잡고 창을 열지 않는다", () => {
    const state = based(3);
    expect(state).toEqual({ base: 3, last: 3, windowEnd: null });
    expect(settleViewers(state).chime).toBeNull();
  });

  it("0 → 1은 입장", () => {
    const state = feed(based(0), [1]);
    expect(state.windowEnd).toBe(1000 + CHIME_WINDOW_MS);
    expect(settleViewers(state)).toEqual({
      state: { base: 1, last: 1, windowEnd: null },
      chime: "join",
    });
  });

  it("2 → 1은 퇴장", () => {
    expect(settleViewers(feed(based(2), [1])).chime).toBe("leave");
  });

  it("창 안의 0 → 1 → 2 → 3은 입장 한 번이고 창은 늘어나지 않는다", () => {
    const state = feed(based(0), [1, 2, 3]);
    expect(state.windowEnd).toBe(1000 + CHIME_WINDOW_MS);
    const settled = settleViewers(state);
    expect(settled.chime).toBe("join");
    expect(settleViewers(settled.state).chime).toBeNull();
  });

  it("창 안의 2 → 1 → 2는 소리가 없다", () => {
    expect(settleViewers(feed(based(2), [1, 2])).chime).toBeNull();
  });

  it("같은 값 반복은 창을 열지 않는다", () => {
    const state = based(2);
    expect(feed(state, [2, 2, 2])).toBe(state);
  });

  it("초기화 뒤 첫 값은 다시 기준값이다", () => {
    expect(feed(based(3), [1]).windowEnd).not.toBeNull();
    const state = observeViewers(initialChimeState(), 0, 2000);
    expect(state).toEqual({ base: 0, last: 0, windowEnd: null });
  });

  it("창이 끝난 뒤의 변화는 새 창을 연다", () => {
    const { state } = settleViewers(feed(based(0), [1]));
    const next = observeViewers(state, 0, 5000);
    expect(next.windowEnd).toBe(5000 + CHIME_WINDOW_MS);
    expect(settleViewers(next).chime).toBe("leave");
  });
});
