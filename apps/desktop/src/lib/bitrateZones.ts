/**
 * 비트레이트 구간("부족"·"적당"·"충분")과 기본값(D-074). 출력 크기·주사율에서 경험식 하나로 계산하고,
 * 조합별 표는 두지 않는다. 값은 모두 kbps(백엔드·IPC와 같은 단위). 영상은 H.264만이다(D-103).
 *
 * 근거(2026-11-22 확인):
 * - YouTube 라이브 인코더 권장 비트레이트 https://support.google.com/youtube/answer/2853702
 *   H.264 최소/권장(Mbps): 1080p30 5/14, 1080p60 6/17, 1440p30 7/21, 1440p60 8/34, 2160p30 11/42, 2160p60 14/50.
 * - 해상도: "Power of .75" 규칙(Ben Waggoner, Jan Ozer 정리)
 *   https://streaminglearningcenter.com/blogs/five-checks-for-your-encoding-ladder.html
 *   화소 수가 k배면 같은 화질에 k^0.75배가 든다(해상도가 클수록 화소당 비트가 덜 든다).
 *   YouTube 권장의 1080p → 2160p(화소 4배)도 2.94~3.0배(지수 0.78~0.79)라 맞는다.
 * - 주사율: 프레임 사이 변화가 작아져서 주사율이 두 배여도 비트레이트는 두 배가 들지 않는다.
 *   YouTube 권장 30 → 60fps는 1.19~1.62배(H.264 세 줄의 기하평균 1.33배 ≈ 2^0.4)라 지수 0.4를 쓴다.
 *   60 → 144fps는 2.4^0.4 ≈ 1.42배다.
 * - 기본값: FHD 60fps H.264에서 7 Mbps(0.1.0 프리셋 기본값). YouTube 최소(6)와 권장(17) 사이에서 낮은 쪽이다.
 *   화면 공유는 정지 화면이 많아 일반 라이브 영상보다 덜 든다.
 * - 최대: 25 Mbps(`MAX_BITRATE_KBPS`, 가장 높은 조합인 QHD 60fps의 바 끝). 화면 공유에서는 그 위로 주어도
 *   화질 이득이 작고 대역폭만 는다.
 * - 바: 왼쪽 끝 = 부족|적당 경계 ÷ 2, 오른쪽 끝 = 기본값 × 5(최대 25 Mbps를 넘지 않는다).
 * - 경계: 부족|적당 = 기본값 × 0.5. 적당|충분은 기본값과 바 오른쪽 끝 사이(로그 눈금)의 같은 자리
 *   (ln 2.5 / ln 5 ≈ 0.57)다. 바 끝이 25 Mbps에 막히지 않으면 기본값 × 2.5이고, 막히면 그만큼 낮아진다.
 *   FHD 60fps 3.5 / 7 / 14.4 Mbps(바 1.75~25), QHD 60fps 5.4 / 10.8 / 17.4 Mbps(바 2.7~25).
 */
import { MAX_BITRATE_KBPS, MIN_BITRATE_KBPS } from "./bitrate";
import type { HostingKind } from "./types";

/** 기준 조합: FHD 60fps. */
export const REFERENCE = { width: 1920, height: 1080, fps: 60 } as const;
/** 기준 조합의 H.264 기본값(kbps). */
export const REFERENCE_KBPS = 7000;
/** 화소 수 지수(Power of .75 규칙). */
export const PIXEL_EXPONENT = 0.75;
/** 주사율 지수(YouTube 권장 30 → 60fps 1.33배). */
export const FPS_EXPONENT = 0.4;
/** 부족|적당 경계 = 기본값 × LOW_RATIO. */
export const LOW_RATIO = 0.5;
/** 바 오른쪽 끝 = 기본값 × BAR_RATIO(최대 비트레이트를 넘지 않는다). */
export const BAR_RATIO = 5;
/** 바 끝이 막히지 않을 때 적당|충분 경계 = 기본값 × HIGH_RATIO. */
export const HIGH_RATIO = 2.5;
/** 적당|충분 경계의 자리: 기본값과 바 오른쪽 끝 사이(로그 눈금)의 이 비율. 바 끝이 기본값 × 5면 기본값 × 2.5다. */
const HIGH_POSITION = Math.log(HIGH_RATIO) / Math.log(BAR_RATIO);

/** 실제로 나갈 화질: 출력 크기, 출력 fps. */
export interface QualityTarget {
  width: number;
  height: number;
  fps: number;
}

/** kbps. low ≤ recommended ≤ high, barMin < barMax. */
export interface BitrateZones {
  /** 이보다 낮으면 "부족" */
  low: number;
  /** "적당" 기본값 */
  recommended: number;
  /** 이보다 높으면 "충분" */
  high: number;
  /** 슬라이드 바의 양 끝(로그 눈금): low/2 ~ 기본값 × 5, 허용 범위(최대 25 Mbps) 안. */
  barMin: number;
  barMax: number;
}

export type BitrateZone = "low" | "ok" | "high";

export const ZONE_LABEL: Record<BitrateZone, string> = {
  low: "부족",
  ok: "적당",
  high: "충분",
};

function clampKbps(kbps: number): number {
  return Math.min(MAX_BITRATE_KBPS, Math.max(MIN_BITRATE_KBPS, kbps));
}

/** 0.1 Mbps(100 kbps) 단위로 반올림하고 허용 범위(0.3~25 Mbps)로 맞춘다. */
function roundKbps(kbps: number): number {
  return clampKbps(Math.round(kbps / 100) * 100);
}

/** 이 화질의 기본값(반올림 전, kbps). */
function baseKbps(t: QualityTarget): number {
  const pixels = Math.max(1, t.width) * Math.max(1, t.height);
  const pixelRatio = pixels / (REFERENCE.width * REFERENCE.height);
  const fps = t.fps > 0 ? t.fps : REFERENCE.fps;
  return REFERENCE_KBPS * pixelRatio ** PIXEL_EXPONENT * (fps / REFERENCE.fps) ** FPS_EXPONENT;
}

/** 출력 크기·주사율로 구간 경계와 기본값을 계산한다. */
export function bitrateZones(t: QualityTarget): BitrateZones {
  const base = baseKbps(t);
  const low = roundKbps(base * LOW_RATIO);
  const recommended = Math.max(low, roundKbps(base));
  const end = roundKbps(base * BAR_RATIO);
  const high = Math.max(recommended, roundKbps(base * Math.max(1, end / base) ** HIGH_POSITION));
  const barMin = clampKbps(Math.round(low / 2 / 10) * 10);
  const barMax = Math.max(barMin + 100, end);
  return { low, recommended, high, barMin, barMax };
}

/** 값이 어느 구간인가. 경계값은 "적당"에 넣는다. */
export function zoneOf(kbps: number, z: Pick<BitrateZones, "low" | "high">): BitrateZone {
  if (kbps < z.low) return "low";
  if (kbps > z.high) return "high";
  return "ok";
}

// ---------- 슬라이드 바(로그 눈금) ----------

/** 바의 칸 수. 한 칸은 바 전체 비율의 약 1.5%(키보드 화살표 한 번). */
export const SLIDER_STEPS = 200;

/** kbps → 바 위치(0~SLIDER_STEPS, 소수). 바 밖의 값은 끝에 붙는다. */
export function sliderPos(kbps: number, z: Pick<BitrateZones, "barMin" | "barMax">): number {
  const lo = Math.log(z.barMin);
  const hi = Math.log(z.barMax);
  const v = Math.log(Math.min(z.barMax, Math.max(z.barMin, kbps)));
  return ((v - lo) / (hi - lo)) * SLIDER_STEPS;
}

/** 바 위치 → kbps. 10 Mbps 미만은 0.1 Mbps, 그 위는 0.5 Mbps 단위로 맞춘다. */
export function sliderKbps(pos: number, z: Pick<BitrateZones, "barMin" | "barMax">): number {
  const lo = Math.log(z.barMin);
  const hi = Math.log(z.barMax);
  const p = Math.min(SLIDER_STEPS, Math.max(0, pos)) / SLIDER_STEPS;
  const kbps = Math.exp(lo + (hi - lo) * p);
  const unit = kbps < 10_000 ? 100 : 500;
  return clampKbps(Math.round(kbps / unit) * unit);
}

// ---------- 글자 ----------

/** 7000 → "7", 17500 → "17.5", 6950 → "6.95"(소수 둘째 자리까지, 끝의 0은 뺀다). */
export function mbpsText(kbps: number): string {
  return String(Math.round(kbps / 10) / 100);
}

/**
 * 바 아래 경고. 적당이면 null.
 * - 부족: 화면이 뭉개진다.
 * - 충분: 요금이 아니라 회선 대역폭 문제다. 시청자마다 이만큼 내려받고, 이 PC에서 호스팅하면(로컬 랜·PC 직접 호스팅)
 *   방송자 업로드가 비트레이트 × 시청자 수가 된다. 전용 서버로 보내면 업로드는 한 번이다.
 * viewers는 방송 중일 때 지금 시청자 수(방송 시작 전이면 null).
 */
export function bitrateWarning(
  zone: BitrateZone,
  kbps: number,
  hosting: HostingKind | null,
  viewers: number | null = null,
): string | null {
  if (zone === "ok") return null;
  if (zone === "low") return "비트레이트가 낮아 움직임이 많은 화면은 뭉개지고 작은 글자는 흐려질 수 있습니다.";
  const m = mbpsText(kbps);
  if (hosting === "lan" || hosting === "direct") {
    const now = viewers && viewers > 0 ? `(지금 ${viewers}명이면 ${mbpsText(kbps * viewers)} Mbps)` : "";
    return `이 PC가 서버라 업로드가 ${m} Mbps × 시청자 수${now}가 됩니다. 회선이 감당하지 못하면 끊길 수 있고, 시청자도 각자 ${m} Mbps를 내려받아야 합니다.`;
  }
  if (hosting === "server") {
    return `시청자마다 ${m} Mbps를 계속 내려받아야 합니다. 회선이 느린 시청자는 끊길 수 있고, 서버도 시청자 수만큼 보내야 합니다.`;
  }
  return `시청자마다 ${m} Mbps를 계속 내려받아야 합니다. 회선이 느리면 끊길 수 있습니다.`;
}
