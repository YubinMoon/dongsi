/**
 * 송출 화면 미리 보기(방송 중 화면의 "다른 사람에게 보이는 모습" 카드, D-066).
 * 백엔드 `broadcast_preview`의 바이너리 응답을 읽는다. 소리는 없다.
 */

/** 미리 보기를 묻는 간격(ms). 백엔드가 만드는 간격(0.1초)과 같다. */
export const PREVIEW_POLL_MS = 100;
/** 창이 가려졌거나 묻기가 실패했을 때 다시 볼 때까지 기다리는 시간(ms) */
export const PREVIEW_IDLE_MS = 500;
/** 응답 머리: 번호·너비·높이(각각 u32 리틀 엔디언) */
const HEADER = 12;

export interface PreviewImage {
  /** 백엔드가 만든 순서. 다음 요청의 `after`로 보낸다. */
  seq: number;
  width: number;
  height: number;
  /** RGBA, 행 간격 = 너비 × 4 */
  rgba: Uint8ClampedArray<ArrayBuffer>;
}

/**
 * 응답을 읽는다. 비었거나(새 그림 없음, 방송 중 아님) 모양이 틀리면 null.
 * Tauri는 바이너리 응답을 보통 ArrayBuffer로 주지만, 다른 경로에서는 바이트 배열로 올 수 있어 둘 다 받는다.
 */
export function decodePreview(data: unknown): PreviewImage | null {
  let bytes: Uint8Array<ArrayBuffer>;
  if (data instanceof ArrayBuffer) bytes = new Uint8Array(data);
  else if (data instanceof Uint8Array) bytes = new Uint8Array(data);
  else if (Array.isArray(data)) bytes = Uint8Array.from(data as number[]);
  else return null;
  if (bytes.byteLength < HEADER) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const seq = view.getUint32(0, true);
  const width = view.getUint32(4, true);
  const height = view.getUint32(8, true);
  const size = width * height * 4;
  if (width === 0 || height === 0 || bytes.byteLength !== HEADER + size) return null;
  return { seq, width, height, rgba: new Uint8ClampedArray(bytes.buffer, bytes.byteOffset + HEADER, size) };
}

/** 카드 크기(CSS 픽셀)와 화면 배율로 요청할 그림 크기(기기 픽셀). 백엔드가 다시 640×640 안으로 줄인다. */
export function previewRequestSize(
  cssWidth: number,
  cssHeight: number,
  dpr: number,
): { width: number; height: number } {
  const scale = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
  const side = (v: number) => (Number.isFinite(v) ? Math.max(16, Math.round(v * scale)) : 16);
  return { width: side(cssWidth), height: side(cssHeight) };
}
