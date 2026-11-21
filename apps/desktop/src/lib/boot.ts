/**
 * 시작 화면(`app.html`의 `#boot`). 앱 JS가 뜨기 전에 WebView2가 처음 그리는 화면이다.
 * 레이아웃이 마운트되면 첫 화면에 필요한 것(서버 목록, 글꼴)을 기다렸다가 흐리게 걷는다.
 * 첫 화면이 "불러오는 중…"이나 대체 글꼴로 한 번 바뀌어 보이지 않게 하려는 것이고, 오래 걸리면 기다리지 않는다.
 */

/** 준비를 기다리는 최대 시간. 넘으면 준비가 덜 됐어도 걷는다. */
export const BOOT_WAIT_MS = 800;
/** 흐려지는 시간. `app.html`의 `#boot` transition과 같게 둔다. */
export const BOOT_FADE_MS = 160;

/** 시작 화면을 걷는 데 쓰는 문서의 최소 모양(시험에서 가짜를 넣는다). */
export interface BootDocument {
  getElementById(id: string): BootElement | null;
}

export interface BootElement {
  classList: { add(name: string): void; contains(name: string): boolean };
  remove(): void;
}

/** `ready`가 모두 끝나거나(실패해도) `maxWaitMs`가 지나면 시작 화면을 걷는다. */
export async function hideBootScreenWhenReady(
  ready: ReadonlyArray<PromiseLike<unknown> | undefined>,
  doc: BootDocument = document,
  maxWaitMs = BOOT_WAIT_MS,
): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<void>((resolve) => {
    timer = setTimeout(resolve, maxWaitMs);
  });
  await Promise.race([Promise.allSettled(ready.filter((p) => p !== undefined)), timeout]);
  clearTimeout(timer);
  hideBootScreen(doc);
}

/** 시작 화면을 흐리게 한 뒤 지운다. 없거나 이미 걷는 중이면 아무것도 하지 않는다. */
export function hideBootScreen(doc: BootDocument = document): void {
  const el = doc.getElementById("boot");
  if (!el || el.classList.contains("hide")) return;
  el.classList.add("hide");
  // transitionend는 창이 숨어 있으면 오지 않을 수 있어 시간으로 지운다.
  setTimeout(() => el.remove(), BOOT_FADE_MS + 60);
}
