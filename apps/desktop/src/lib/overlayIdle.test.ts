import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isOverlayKey, OVERLAY_HIDE_AFTER_MS, OverlayIdle } from "./overlayIdle";

/** 가짜 시계(vi 타이머) 위의 OverlayIdle과 onchange 기록 */
function make(hideAfterMs?: number) {
  const changes: boolean[] = [];
  const idle = new OverlayIdle({ onchange: (h) => changes.push(h), hideAfterMs });
  return { idle, changes };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("OverlayIdle", () => {
  it("기본은 3초", () => {
    expect(OVERLAY_HIDE_AFTER_MS).toBe(3000);
  });

  it("재생이 시작되면 3초 뒤 숨긴다", () => {
    const { idle, changes } = make();
    expect(idle.hidden).toBe(false);
    idle.setEnabled(true);
    vi.advanceTimersByTime(2999);
    expect(idle.hidden).toBe(false);
    vi.advanceTimersByTime(1);
    expect(idle.hidden).toBe(true);
    expect(changes).toEqual([true]);
  });

  it("재생 중이 아니면 숨기지 않는다", () => {
    const { idle, changes } = make();
    idle.pointerMoved(10, 10);
    vi.advanceTimersByTime(60_000);
    expect(idle.hidden).toBe(false);
    expect(changes).toEqual([]);
  });

  it("마우스를 움직이면 처음부터 다시 잰다", () => {
    const { idle } = make();
    idle.setEnabled(true);
    vi.advanceTimersByTime(2000);
    idle.pointerMoved(10, 10);
    vi.advanceTimersByTime(2000);
    expect(idle.hidden).toBe(false);
    vi.advanceTimersByTime(1000);
    expect(idle.hidden).toBe(true);
  });

  it("숨긴 뒤 마우스를 움직이면 다시 보인다", () => {
    const { idle, changes } = make();
    idle.setEnabled(true);
    vi.advanceTimersByTime(3000);
    idle.pointerMoved(5, 5);
    expect(idle.hidden).toBe(false);
    expect(changes).toEqual([true, false]);
    vi.advanceTimersByTime(3000);
    expect(idle.hidden).toBe(true);
    expect(changes).toEqual([true, false, true]);
  });

  it("같은 자리의 pointermove는 움직임으로 세지 않는다(커서를 감출 때 오는 가짜 이벤트)", () => {
    const { idle } = make();
    idle.setEnabled(true);
    idle.pointerMoved(100, 200);
    vi.advanceTimersByTime(3000);
    expect(idle.hidden).toBe(true);
    idle.pointerMoved(100, 200);
    expect(idle.hidden).toBe(true);
    idle.pointerMoved(101, 200);
    expect(idle.hidden).toBe(false);
  });

  it("누름(activity)도 다시 보이게 한다", () => {
    const { idle } = make();
    idle.setEnabled(true);
    vi.advanceTimersByTime(3000);
    idle.activity();
    expect(idle.hidden).toBe(false);
    vi.advanceTimersByTime(2999);
    expect(idle.hidden).toBe(false);
  });

  it("재생이 끝나면 바로 보이고, 다시 재생되면 그때부터 잰다", () => {
    const { idle } = make();
    idle.setEnabled(true);
    vi.advanceTimersByTime(3000);
    expect(idle.hidden).toBe(true);
    idle.setEnabled(false);
    expect(idle.hidden).toBe(false);
    vi.advanceTimersByTime(10_000);
    expect(idle.hidden).toBe(false);
    idle.setEnabled(true);
    vi.advanceTimersByTime(2999);
    expect(idle.hidden).toBe(false);
    vi.advanceTimersByTime(1);
    expect(idle.hidden).toBe(true);
  });

  it("같은 값으로 setEnabled를 다시 불러도 다시 재지 않는다", () => {
    const { idle } = make();
    idle.setEnabled(true);
    vi.advanceTimersByTime(2000);
    idle.setEnabled(true);
    vi.advanceTimersByTime(1000);
    expect(idle.hidden).toBe(true);
  });

  it("막대 위에 마우스가 있으면 숨기지 않고, 벗어나면 그때부터 3초", () => {
    const { idle } = make();
    idle.setEnabled(true);
    idle.hold("pointer", true);
    vi.advanceTimersByTime(60_000);
    expect(idle.hidden).toBe(false);
    idle.hold("pointer", false);
    vi.advanceTimersByTime(2999);
    expect(idle.hidden).toBe(false);
    vi.advanceTimersByTime(1);
    expect(idle.hidden).toBe(true);
  });

  it("이유가 여럿이면 모두 풀려야 잰다", () => {
    const { idle } = make();
    idle.setEnabled(true);
    idle.hold("pointer", true);
    idle.hold("drag", true);
    idle.hold("pointer", false);
    vi.advanceTimersByTime(10_000);
    expect(idle.hidden).toBe(false);
    idle.hold("drag", false);
    vi.advanceTimersByTime(3000);
    expect(idle.hidden).toBe(true);
  });

  it("숨긴 상태에서 키보드 포커스가 막대로 들어오면 바로 보인다", () => {
    const { idle, changes } = make();
    idle.setEnabled(true);
    vi.advanceTimersByTime(3000);
    idle.hold("focus", true);
    expect(idle.hidden).toBe(false);
    expect(changes).toEqual([true, false]);
    vi.advanceTimersByTime(10_000);
    expect(idle.hidden).toBe(false);
  });

  it("이유를 같은 값으로 다시 걸거나 풀어도 바뀌지 않는다", () => {
    const { idle } = make();
    idle.setEnabled(true);
    vi.advanceTimersByTime(2000);
    // 걸지 않은 이유를 풀어도 다시 재지 않는다.
    idle.hold("focus", false);
    vi.advanceTimersByTime(1000);
    expect(idle.hidden).toBe(true);
    idle.hold("drag", true);
    idle.hold("drag", true);
    idle.hold("drag", false);
    vi.advanceTimersByTime(3000);
    expect(idle.hidden).toBe(true);
  });

  it("재생 전에 건 이유는 재생이 시작돼도 남는다", () => {
    const { idle } = make();
    idle.hold("pointer", true);
    idle.setEnabled(true);
    vi.advanceTimersByTime(10_000);
    expect(idle.hidden).toBe(false);
    idle.hold("pointer", false);
    vi.advanceTimersByTime(3000);
    expect(idle.hidden).toBe(true);
  });

  it("마우스를 움직여도 이유가 걸려 있으면 숨기지 않는다", () => {
    const { idle } = make();
    idle.setEnabled(true);
    idle.hold("focus", true);
    idle.pointerMoved(1, 1);
    vi.advanceTimersByTime(10_000);
    expect(idle.hidden).toBe(false);
  });

  it("dispose 뒤에는 숨기지 않는다", () => {
    const { idle, changes } = make();
    idle.setEnabled(true);
    idle.dispose();
    vi.advanceTimersByTime(10_000);
    expect(changes).toEqual([]);
  });

  it("숨기는 시간을 바꿀 수 있다", () => {
    const { idle } = make(500);
    idle.setEnabled(true);
    vi.advanceTimersByTime(500);
    expect(idle.hidden).toBe(true);
  });
});

describe("isOverlayKey", () => {
  const key = (k: string, mods: Partial<{ ctrlKey: boolean; altKey: boolean; metaKey: boolean }> = {}) => ({
    key: k,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    ...mods,
  });
  it("포커스 이동·누름·음량 조절 키", () => {
    for (const k of ["Tab", "Enter", " ", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]) {
      expect(isOverlayKey(key(k))).toBe(true);
    }
  });
  it("단축키(M·F)와 수정 키만 누른 것은 아니다", () => {
    for (const k of ["m", "M", "f", "F", "Alt", "Shift", "Control", "Meta", "Escape", "a"]) {
      expect(isOverlayKey(key(k))).toBe(false);
    }
  });
  it("Shift+Tab은 맞고, Ctrl·Alt·Meta 조합은 아니다", () => {
    expect(isOverlayKey({ ...key("Tab"), shiftKey: true } as ReturnType<typeof key>)).toBe(true);
    expect(isOverlayKey(key("Tab", { altKey: true }))).toBe(false);
    expect(isOverlayKey(key("Tab", { ctrlKey: true }))).toBe(false);
    expect(isOverlayKey(key("ArrowUp", { metaKey: true }))).toBe(false);
  });
});
