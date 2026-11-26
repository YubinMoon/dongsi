import { isTypingTarget } from "./watch";

// 시청 화면의 ESC: 위쪽 막대의 "나가기"(분리 창은 "창 닫기")와 같게 동작한다. 여기는 언제 그러는지만 정한다.

/** ESC로 닫히는 팝업을 찾는 선택자: 열린 대화 상자와 펼친 메뉴. 닫는 일은 그 요소(또는 브라우저)가 한다. */
export const ESCAPE_POPUP_SELECTOR = 'dialog[open], [role="menu"]';

/** keydown에서 보는 값(`KeyboardEvent`를 그대로 넘긴다) */
export interface EscapeKey {
  key: string;
  repeat: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  /** 앞의 처리기(Menu 등)가 이 키를 이미 썼다 */
  defaultPrevented: boolean;
  /** 한글 입력기 등이 조합 중이다 */
  isComposing: boolean;
}

/** 키를 누른 순간의 화면 상태 */
export interface EscapeScreen {
  fullscreen: boolean;
  /** `ESCAPE_POPUP_SELECTOR`에 맞는 것이 있다 */
  popupOpen: boolean;
  /** 키를 받은 요소. HTML 요소가 아니면 null */
  focus: { tagName: string; inputType: string | null; contentEditable: boolean } | null;
  /** 통계 패널이 화면에 보인다 */
  statsOpen: boolean;
}

/**
 * - `none`: 아무것도 하지 않는다(ESC가 아니거나, 다른 곳이 이 키를 쓴다).
 * - `exitFullscreen`: 전체 화면만 푼다.
 * - `closeStats`: 통계 패널만 닫는다.
 * - `leave`: 나가기(분리 창은 창 닫기).
 */
export type EscapeAction = "none" | "exitFullscreen" | "closeStats" | "leave";

/** ESC 한 번이 할 일. 한 번에 한 가지만 한다. */
export function escapeAction(key: EscapeKey, screen: EscapeScreen): EscapeAction {
  if (key.key !== "Escape" || key.repeat || key.defaultPrevented || key.isComposing) return "none";
  if (key.ctrlKey || key.altKey || key.shiftKey || key.metaKey) return "none";
  if (screen.fullscreen) return "exitFullscreen";
  if (screen.popupOpen) return "none";
  const focus = screen.focus;
  if (focus && (focus.contentEditable || isTypingTarget(focus.tagName, focus.inputType))) return "none";
  if (screen.statsOpen) return "closeStats";
  return "leave";
}
