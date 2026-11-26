// 시청 쪽 지연: 서버에서 화면에 나오기까지 걸리는 시간의 추정치(ms).
// RTT/2 + 비디오 지터 버퍼 지연 + 프레임당 디코딩 시간. 모두 시청 PC에서 잰 길이라 PC 간 시계 차이와 무관하다.
// 방송자 쪽 캡처·인코딩과 방송자→서버 구간은 빠진다.

import type { WatchStats } from "./watchStats";

/** 패널에 붙이는 설명 */
export const VIEWER_LATENCY_HINT =
  "서버에서 화면에 나오기까지. 방송자 쪽 캡처·인코딩과 서버까지의 전송은 빠진다.";

/** 구성 값 중 하나라도 없으면 null. 합은 반올림하지 않은 값끼리 더한 뒤 정수 ms로 반올림한다. */
export function viewerLatencyMs(
  stats: Pick<WatchStats, "rttMs" | "videoJitterMs" | "decodeMsPerFrame">,
): number | null {
  const { rttMs, videoJitterMs, decodeMsPerFrame } = stats;
  if (rttMs === null || videoJitterMs === null || decodeMsPerFrame === null) return null;
  return Math.round(rttMs / 2 + videoJitterMs + decodeMsPerFrame);
}
