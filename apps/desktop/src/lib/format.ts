/** 세로 1080 → "1080p · 60fps", 2160 이상 → "4K · 30fps" */
export function qualityLabel(height: number, fps: number): string {
  const tier = height >= 2160 ? "4K" : `${height}p`;
  return `${tier} · ${fps}fps`;
}

/** 초를 시계 모양으로: 5 → "0:05", 754 → "12:34", 3723 → "1:02:03". 음수·NaN은 0으로 본다. */
export function formatClock(seconds: number): string {
  const total = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, "0");
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${ss}`;
  return `${m}:${ss}`;
}

/** 방송한 지 얼마나 됐는지: 60초 미만 "방금", "N분", 정시면 "N시간", 아니면 "N시간 M분". */
export function formatAge(seconds: number): string {
  const total = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  if (total < 60) return "방금";
  if (total < 3600) return `${Math.floor(total / 60)}분`;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  return m === 0 ? `${h}시간` : `${h}시간 ${m}분`;
}

/** 유닉스 시각(초) 이후 지난 초. 시계가 어긋나 음수가 되면 0. */
export function secondsSince(startedAtUnixSec: number, nowMs: number): number {
  return Math.max(0, Math.floor(nowMs / 1000 - startedAtUnixSec));
}

/** 주소 목록 요약: "a" / "a 외 2개" / 빈 배열이면 "". */
export function addrSummary(addrs: string[]): string {
  if (addrs.length === 0) return "";
  if (addrs.length === 1) return addrs[0];
  return `${addrs[0]} 외 ${addrs.length - 1}개`;
}

export type Via = "lan" | "tailscale" | "local";

const VIA_LABELS: Record<Via, string> = {
  lan: "LAN",
  tailscale: "Tailscale",
  local: "이 PC",
};

export function viaLabel(via: Via): string {
  return VIA_LABELS[via] ?? via;
}

const segmenter = typeof Intl !== "undefined" && "Segmenter" in Intl ? new Intl.Segmenter() : null;

/**
 * 이름의 첫 글자 하나. 서로게이트 쌍·결합 문자·이모지를 깨뜨리지 않도록 자소 단위로 자르고,
 * 라틴 글자는 대문자로 바꾼다. 비어 있으면 "?".
 */
export function initial(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) return "?";
  let first: string;
  if (segmenter) {
    first = segmenter.segment(trimmed)[Symbol.iterator]().next().value?.segment ?? "";
  } else {
    first = Array.from(trimmed)[0] ?? "";
  }
  if (!first) return "?";
  return /^\p{Script=Latin}/u.test(first) ? first.toLocaleUpperCase() : first;
}

/** 숫자 표시. null·undefined·NaN·무한대는 "–". */
export function fmt(n: number | null | undefined, digits = 0): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "–";
  const text = n.toFixed(digits);
  return /^-0(\.0+)?$/.test(text) ? text.slice(1) : text;
}
