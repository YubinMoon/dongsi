import { api } from "$lib/api";
import { receivableFrom } from "$lib/broadcast";
import type { EncoderView } from "$lib/types";

/**
 * 방송 화질을 고를 때 필요한 것: 인코더 목록, 시청 앱(WebView2)이 재생할 수 있는 코덱.
 * 방송 시작 화면과 방송 중 "화질 변경"이 각자 하나씩 만들어 쓴다.
 */
export class Capabilities {
  /** null이면 아직 확인 중 */
  encoders = $state<EncoderView[] | null>(null);
  /** 재생할 수 있는 mimeType(소문자). 읽지 못하면 빈 집합 */
  receivable = $state.raw<Set<string>>(new Set());

  readReceivable() {
    try {
      this.receivable = receivableFrom(RTCRtpReceiver.getCapabilities("video"));
    } catch {
      this.receivable = new Set();
    }
  }

  async loadEncoders() {
    try {
      this.encoders = await api.availableEncoders();
    } catch {
      this.encoders = [];
    }
  }
}
