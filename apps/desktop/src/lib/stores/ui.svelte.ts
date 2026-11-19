/** 화면 전체에 걸친 작은 상태. */
export const ui = $state({
  /** 서버 추가 대화 상자(위쪽 바 +, 홈의 "서버 추가"가 연다) */
  addServerOpen: false,
});

export function openAddServer() {
  ui.addServerOpen = true;
}
