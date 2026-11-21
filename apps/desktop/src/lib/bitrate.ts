/**
 * 비트레이트 단위. 백엔드·IPC·state.json은 정수 kbps(`bitrateKbps`), 화면은 Mbps로 소수 둘째 자리(10 kbps)까지 보인다.
 */
import { fmt } from "./format";

/** 입력 범위(Mbps). Rust `VideoSettings::MIN_BITRATE_KBPS`(300)·`MAX_BITRATE_KBPS`(100_000)와 같다. 범위를 넘는 값은 백엔드가 맞춘다. */
export const MIN_BITRATE_MBPS = 0.3;
export const MAX_BITRATE_MBPS = 100;
export const MIN_BITRATE_KBPS = 300;
export const MAX_BITRATE_KBPS = 100_000;

/** kbps → 입력란에 넣을 Mbps(소수 둘째 자리). 숫자가 아니면 null(빈 칸). */
export function kbpsToMbps(kbps: number | null | undefined): number | null {
  if (typeof kbps !== "number" || !Number.isFinite(kbps)) return null;
  return Math.round(kbps / 10) / 100;
}

/** 입력란의 Mbps → kbps. 소수 둘째 자리(10 kbps)로 반올림한다. 비었으면(null) null. */
export function mbpsToKbps(mbps: number | null | undefined): number | null {
  if (typeof mbps !== "number" || !Number.isFinite(mbps)) return null;
  return Math.round(mbps * 100) * 10;
}

/** 통계 표시: 6950 → "6.95 Mbps". 값이 없으면 "– Mbps". */
export function fmtMbps(kbps: number | null | undefined): string {
  return `${fmt(typeof kbps === "number" ? kbps / 1000 : kbps, 2)} Mbps`;
}

/**
 * 입력란에서 확정한 값(비었으면 null)을 보낼 kbps로. 숫자가 아니거나 0 이하면 fallback,
 * 아니면 10 kbps 단위로 반올림해 허용 범위(0.3~100 Mbps) 안으로 맞춘다(넘으면 백엔드도 같은 값으로 맞춘다).
 */
export function normalizeKbps(value: number | null | undefined, fallback: number): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) return fallback;
  return Math.min(MAX_BITRATE_KBPS, Math.max(MIN_BITRATE_KBPS, Math.round(value / 10) * 10));
}
