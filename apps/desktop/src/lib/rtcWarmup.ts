// 첫 시청 예열: WebView2(Chromium)에서 렌더러의 첫 `new RTCPeerConnection()`은 약 350ms가 걸리고(메인 스레드를 막는다)
// 두 번째부터는 1ms 미만이다(2026-11-20 팀장 측정; createOffer·setLocalDescription은 처음에도 2~4ms). 첫 시청의 "클릭 → 첫 프레임"이
// 1.1~1.5초(그다음부터 0.3~0.4초)인 원인 중 하나라, 앱이 한가할 때 PC 하나를 만들었다가 바로 닫아 이 비용을 미리 낸다.
//
// createOffer·setLocalDescription은 부르지 않는다: 소켓을 열거나 후보 수집을 시작하지 않고 생성자만 데운다.
// 순수하게 시험할 수 있게 생성자를 인자로 받는다.

/** 예열에 필요한 만큼만 본 RTCPeerConnection 생성자 */
export type PeerConnectionCtor = new () => { close(): void };

/** 창(문서)마다 한 번만 한다. 시도했으면(생성자가 던졌어도) 다시 하지 않는다. */
let warmed = false;

/**
 * PC 하나를 만들고 바로 닫는다. 창마다 한 번만 하고, 예외는 삼킨다.
 * 생성자가 없으면(WebRTC가 없는 환경) 아무것도 하지 않고, 나중에 생성자가 있는 호출은 여전히 한 번 한다.
 * 이번 호출에서 PC를 만들었으면 true.
 */
export function warmUpWebRtc(Ctor: PeerConnectionCtor | undefined = globalThis.RTCPeerConnection): boolean {
  if (warmed || typeof Ctor !== "function") return false;
  warmed = true;
  try {
    const pc = new Ctor();
    try {
      pc.close();
    } catch {}
    return true;
  } catch {
    return false;
  }
}

/** 시험용: "이미 했음" 표시를 지운다. */
export function resetWarmUpForTests(): void {
  warmed = false;
}
