// 시청 화면 위·아래 막대(오버레이)의 자동 숨김(D-077).
//
// 재생 중에 마우스를 3초 움직이지 않으면 막대를 숨긴다(시청 페이지가 투명도로 숨기고 영상 위 커서도 감춘다).
// 마우스를 움직이거나 누르면 다시 보이고 처음부터 다시 잰다.
// 막대 위에 마우스가 있거나(pointer), 막대 안에 키보드 포커스가 있거나(focus), 음량 바를 끄는 중이면(drag) 숨기지 않는다.
// 이런 이유가 모두 풀리면 그때부터 다시 잰다. 재생 중이 아니면(연결 중·비밀번호·끝남 등) 늘 보인다.

/** 마우스를 움직이지 않은 채 이만큼 지나면 숨긴다(ms). */
export const OVERLAY_HIDE_AFTER_MS = 3000;

/** 숨기지 않는 이유: 막대 위 마우스, 막대 안 키보드 포커스, 음량 바 끄는 중 */
export type OverlayHold = "pointer" | "focus" | "drag";

export interface OverlayIdleOptions {
  /** 보임·숨김이 바뀔 때 부른다(숨기면 true). 같은 값으로는 다시 부르지 않는다. */
  onchange: (hidden: boolean) => void;
  hideAfterMs?: number;
  /** 시험용: 타이머(기본은 setTimeout·clearTimeout) */
  setTimer?: (fn: () => void, ms: number) => unknown;
  clearTimer?: (id: unknown) => void;
}

export class OverlayIdle {
  #hidden = false;
  #enabled = false;
  readonly #holds = new Set<OverlayHold>();
  #timer: unknown = null;
  /** 직전 pointermove 좌표. 같은 자리의 이벤트는 움직임으로 세지 않는다. */
  #last: { x: number; y: number } | null = null;
  readonly #onchange: (hidden: boolean) => void;
  readonly #hideAfterMs: number;
  readonly #setTimer: (fn: () => void, ms: number) => unknown;
  readonly #clearTimer: (id: unknown) => void;

  constructor(opts: OverlayIdleOptions) {
    this.#onchange = opts.onchange;
    this.#hideAfterMs = opts.hideAfterMs ?? OVERLAY_HIDE_AFTER_MS;
    this.#setTimer = opts.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
    this.#clearTimer = opts.clearTimer ?? ((id) => clearTimeout(id as ReturnType<typeof setTimeout>));
  }

  get hidden(): boolean {
    return this.#hidden;
  }

  /** 재생 중인가. 아니면 바로 보이고 재지 않는다. 재생이 시작되면 그때부터 잰다. */
  setEnabled(on: boolean): void {
    if (on === this.#enabled) return;
    this.#enabled = on;
    if (on) {
      this.#restart();
    } else {
      this.#cancel();
      this.#set(false);
    }
  }

  /**
   * 마우스가 움직였다(pointermove의 좌표). 직전과 같은 자리면 세지 않는다:
   * Chromium은 커서 모양이나 아래 요소가 바뀔 때 움직이지 않아도 mousemove를 보낼 수 있어서,
   * 숨기자마자(커서를 감추자마자) 다시 보이는 일을 막는다.
   */
  pointerMoved(x: number, y: number): void {
    if (this.#last !== null && this.#last.x === x && this.#last.y === y) return;
    this.#last = { x, y };
    this.activity();
  }

  /** 사용자가 무언가 했다(마우스 움직임·누름): 보이고 처음부터 다시 잰다. */
  activity(): void {
    this.#set(false);
    this.#restart();
  }

  /** 숨기지 않는 이유를 걸거나 푼다. 걸면 바로 보이고, 모두 풀리면 그때부터 다시 잰다. */
  hold(reason: OverlayHold, on: boolean): void {
    if (on === this.#holds.has(reason)) return;
    if (on) {
      this.#holds.add(reason);
      this.#cancel();
      this.#set(false);
    } else {
      this.#holds.delete(reason);
      this.#restart();
    }
  }

  /** 타이머를 치운다(화면을 떠날 때). */
  dispose(): void {
    this.#cancel();
  }

  #restart(): void {
    this.#cancel();
    if (!this.#enabled || this.#holds.size > 0) return;
    this.#timer = this.#setTimer(() => {
      this.#timer = null;
      this.#set(true);
    }, this.#hideAfterMs);
  }

  #cancel(): void {
    if (this.#timer === null) return;
    this.#clearTimer(this.#timer);
    this.#timer = null;
  }

  #set(hidden: boolean): void {
    if (hidden === this.#hidden) return;
    this.#hidden = hidden;
    this.#onchange(hidden);
  }
}

/** 키보드로 막대를 다루는 키: 포커스 이동(Tab)·누름(Enter·Space)·음량 조절(화살표 등). */
const OVERLAY_KEYS = new Set([
  "Tab",
  "Enter",
  " ",
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "Home",
  "End",
  "PageUp",
  "PageDown",
]);

/**
 * 이 키가 "키보드로 막대를 다루는" 입력인가. 시청 페이지는 이런 키 뒤에 막대 안으로 들어온 포커스를 키보드 포커스로 보고
 * 막대를 숨기지 않는다. 단축키(M·F), 수정 키만 누른 것(Alt+Tab의 Alt 등), Ctrl·Alt·Meta 조합은 아니다
 * (마우스로 단추를 누른 뒤 M을 눌렀다고 막대가 계속 남지 않게).
 */
export function isOverlayKey(event: { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean }): boolean {
  if (event.ctrlKey || event.altKey || event.metaKey) return false;
  return OVERLAY_KEYS.has(event.key);
}
