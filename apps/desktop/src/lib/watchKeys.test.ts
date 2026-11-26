import { describe, expect, it } from "vitest";
import { escapeAction, type EscapeKey, type EscapeScreen } from "./watchKeys";

const esc = (over: Partial<EscapeKey> = {}): EscapeKey => ({
  key: "Escape",
  repeat: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  defaultPrevented: false,
  isComposing: false,
  ...over,
});

const screen = (over: Partial<EscapeScreen> = {}): EscapeScreen => ({
  fullscreen: false,
  popupOpen: false,
  focus: null,
  statsOpen: false,
  ...over,
});

const focus = (tagName: string, inputType: string | null = null, contentEditable = false) => ({
  tagName,
  inputType,
  contentEditable,
});

describe("escapeAction", () => {
  it("가리는 것이 없으면 나간다", () => {
    expect(escapeAction(esc(), screen())).toBe("leave");
  });

  it("ESC가 아닌 키는 아무것도 하지 않는다", () => {
    for (const key of ["Enter", "f", "m", "Backspace"]) {
      expect(escapeAction(esc({ key }), screen())).toBe("none");
    }
  });

  it("키 반복·조합 키·입력기 조합 중·이미 쓴 키는 무시한다", () => {
    const ignored: Partial<EscapeKey>[] = [
      { repeat: true },
      { ctrlKey: true },
      { altKey: true },
      { shiftKey: true },
      { metaKey: true },
      { isComposing: true },
      { defaultPrevented: true },
    ];
    for (const over of ignored) {
      expect(escapeAction(esc(over), screen())).toBe("none");
      expect(escapeAction(esc(over), screen({ fullscreen: true, statsOpen: true }))).toBe("none");
    }
  });

  it("전체 화면이면 다른 것이 열려 있어도 전체 화면만 푼다", () => {
    expect(escapeAction(esc(), screen({ fullscreen: true }))).toBe("exitFullscreen");
    expect(escapeAction(esc(), screen({ fullscreen: true, statsOpen: true, popupOpen: true }))).toBe("exitFullscreen");
  });

  it("대화 상자·메뉴가 열려 있으면 그쪽에 맡긴다", () => {
    expect(escapeAction(esc(), screen({ popupOpen: true }))).toBe("none");
    expect(escapeAction(esc(), screen({ popupOpen: true, statsOpen: true }))).toBe("none");
  });

  it("입력란에 포커스가 있으면 나가지 않는다", () => {
    for (const f of [
      focus("INPUT", "text"),
      focus("INPUT", "password"),
      focus("TEXTAREA"),
      focus("SELECT"),
      focus("DIV", null, true),
    ]) {
      expect(escapeAction(esc(), screen({ focus: f }))).toBe("none");
    }
  });

  it("음량 슬라이더·체크 상자·단추에 포커스가 있으면 나간다", () => {
    for (const f of [focus("INPUT", "range"), focus("INPUT", "checkbox"), focus("BUTTON"), focus("BODY")]) {
      expect(escapeAction(esc(), screen({ focus: f }))).toBe("leave");
    }
  });

  it("통계 패널이 열려 있으면 패널만 닫고, 다음 ESC에서 나간다", () => {
    expect(escapeAction(esc(), screen({ statsOpen: true }))).toBe("closeStats");
    expect(escapeAction(esc(), screen({ statsOpen: false }))).toBe("leave");
  });
});
