import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BOOT_FADE_MS, BOOT_WAIT_MS, hideBootScreen, hideBootScreenWhenReady, type BootDocument } from "./boot";

function fakeDoc(present = true) {
  const classes = new Set<string>();
  const el = {
    classList: { add: (c: string) => void classes.add(c), contains: (c: string) => classes.has(c) },
    remove: vi.fn(),
  };
  const doc: BootDocument = { getElementById: (id) => (present && id === "boot" ? el : null) };
  return { doc, el, classes };
}

describe("시작 화면 걷기", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("준비가 끝나면 흐리게 한 뒤 지운다", async () => {
    const { doc, el, classes } = fakeDoc();
    let done!: () => void;
    const ready = new Promise<void>((r) => (done = r));
    const p = hideBootScreenWhenReady([ready], doc);
    await vi.advanceTimersByTimeAsync(100);
    expect(classes.has("hide")).toBe(false);
    done();
    await p;
    expect(classes.has("hide")).toBe(true);
    expect(el.remove).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(BOOT_FADE_MS + 60);
    expect(el.remove).toHaveBeenCalledOnce();
  });

  it("준비가 실패해도 걷는다", async () => {
    const { doc, classes } = fakeDoc();
    await hideBootScreenWhenReady([Promise.reject(new Error("x")), undefined], doc);
    expect(classes.has("hide")).toBe(true);
  });

  it("준비가 오래 걸리면 기다리지 않는다", async () => {
    const { doc, classes } = fakeDoc();
    const p = hideBootScreenWhenReady([new Promise(() => {})], doc);
    await vi.advanceTimersByTimeAsync(BOOT_WAIT_MS - 1);
    expect(classes.has("hide")).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await p;
    expect(classes.has("hide")).toBe(true);
  });

  it("없거나 이미 걷는 중이면 아무것도 하지 않는다", async () => {
    hideBootScreen(fakeDoc(false).doc);
    const { doc, el } = fakeDoc();
    hideBootScreen(doc);
    hideBootScreen(doc);
    await vi.advanceTimersByTimeAsync(BOOT_FADE_MS + 60);
    expect(el.remove).toHaveBeenCalledOnce();
  });
});
