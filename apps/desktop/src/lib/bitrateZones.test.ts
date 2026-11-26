import { describe, expect, it } from "vitest";
import {
  HIGH_RATIO,
  LOW_RATIO,
  SLIDER_STEPS,
  bitrateWarning,
  bitrateZones,
  mbpsText,
  sliderKbps,
  sliderPos,
  zoneOf,
  type QualityTarget,
} from "./bitrateZones";
import { qualityTarget } from "./broadcast";

const t = (width: number, height: number, fps: number): QualityTarget => ({ width, height, fps });
const FHD = [1920, 1080] as const;
const QHD = [2560, 1440] as const;
const UHD = [3840, 2160] as const;

describe("bitrateZones", () => {
  it("기준 FHD 60fps H.264: 3.5 / 7 / 17.5 Mbps", () => {
    expect(bitrateZones(t(...FHD, 60))).toMatchObject({ low: 3500, recommended: 7000, high: 17_500 });
  });

  it("경계 순서: 부족 < 적당 < 충분, 기본값은 적당 구간 안", () => {
    for (const size of [[1280, 720], FHD, QHD, UHD, [3440, 1440], [7680, 4320]] as const) {
      for (const fps of [30, 60, 75, 144, 240]) {
        const z = bitrateZones(t(size[0], size[1], fps));
        expect(z.low).toBeLessThan(z.recommended);
        expect(z.recommended).toBeLessThan(z.high);
        expect(zoneOf(z.recommended, z)).toBe("ok");
        // 허용 범위 끝(0.3 / 100 Mbps)에 닿으면 바 끝과 경계가 같을 수 있다.
        expect(z.barMin).toBeLessThanOrEqual(z.low);
        expect(z.barMax).toBeGreaterThanOrEqual(z.high);
        if (z.high < 50_000) expect(z.barMax).toBeGreaterThan(z.high);
        if (z.low > 600) expect(z.barMin).toBeLessThan(z.low);
        expect(z.barMin).toBeGreaterThanOrEqual(300);
        expect(z.barMax).toBeLessThanOrEqual(100_000);
      }
    }
  });

  it("해상도: QHD > FHD, 4K > QHD. 화소 수만큼은 늘지 않는다(Power of .75)", () => {
    const fhd = bitrateZones(t(...FHD, 60)).recommended;
    const qhd = bitrateZones(t(...QHD, 60)).recommended;
    const uhd = bitrateZones(t(...UHD, 60)).recommended;
    expect(qhd).toBeGreaterThan(fhd);
    expect(uhd).toBeGreaterThan(qhd);
    // 4K는 화소 4배지만 4^0.75 ≈ 2.83배
    expect(uhd / fhd).toBeCloseTo(2.83, 1);
    expect(uhd / fhd).toBeLessThan(4);
  });

  it("주사율: 144 > 60 > 30, 비례보다 덜 는다", () => {
    const at = (fps: number) => bitrateZones(t(...FHD, fps)).recommended;
    expect(at(144)).toBeGreaterThan(at(60));
    expect(at(60)).toBeGreaterThan(at(30));
    // 30 → 60(2배)은 2^0.4 ≈ 1.32배, 60 → 144(2.4배)는 2.4^0.4 ≈ 1.42배
    expect(at(60) / at(30)).toBeCloseTo(1.32, 1);
    expect(at(60) / at(30)).toBeLessThan(2);
    expect(at(144) / at(60)).toBeCloseTo(1.42, 1);
    expect(at(144) / at(60)).toBeLessThan(144 / 60);
  });

  it("충분 경계는 YouTube 권장(일반 라이브)과 ±25% 안, 기본값은 그보다 훨씬 낮다", () => {
    // https://support.google.com/youtube/answer/2853702 (2026-11-22 확인) 권장 Mbps
    // H.264 줄(D-103: 영상은 H.264만)
    const youtube: [number, number, number, number][] = [
      [...FHD, 30, 14],
      [...FHD, 60, 17],
      [...QHD, 30, 21],
      [...UHD, 30, 42],
      [...UHD, 60, 50],
    ];
    for (const [w, h, fps, mbps] of youtube) {
      const z = bitrateZones(t(w, h, fps));
      expect(Math.abs(z.high / 1000 / mbps - 1)).toBeLessThan(0.25);
      expect(z.recommended).toBeLessThan(mbps * 1000 * 0.5);
    }
  });

  it("경계는 기본값의 0.5배·2.5배", () => {
    const z = bitrateZones(t(...QHD, 144));
    expect(z.low / z.recommended).toBeCloseTo(LOW_RATIO, 1);
    expect(z.high / z.recommended).toBeCloseTo(HIGH_RATIO, 1);
  });

  it("0.1 Mbps 단위로 반올림하고 허용 범위(0.3~100 Mbps) 안", () => {
    const tiny = bitrateZones(t(160, 90, 30));
    expect(tiny.low).toBe(300);
    expect(tiny.barMin).toBe(300);
    const huge = bitrateZones(t(7680, 4320, 240));
    expect(huge.high).toBe(100_000);
    expect(huge.barMax).toBe(100_000);
    for (const z of [tiny, huge, bitrateZones(t(...QHD, 60))]) {
      for (const v of [z.low, z.recommended, z.high]) expect(v % 100).toBe(0);
    }
  });
});

describe("출력 크기·주사율이 구간에 들어가는 방식", () => {
  const monitor = (width: number, height: number, refreshRate: number | null) => ({ width, height, refreshRate });

  it("QHD 상자면 4K 화면도 QHD로 계산한다(4K 없음, D-097)", () => {
    const q = { width: 2560, height: 1440, fps: 60 };
    expect(bitrateZones(qualityTarget(q, monitor(3840, 2160, 60)))).toEqual(bitrateZones(t(...QHD, 60)));
    expect(bitrateZones(qualityTarget(q, monitor(2560, 1440, 60)))).toEqual(bitrateZones(t(...QHD, 60)));
  });

  it("FHD 상자에 원본이 더 작으면 원본 크기로(1280×720 창은 FHD보다 낮다)", () => {
    const q = { width: 1920, height: 1080, fps: 60 };
    const small = bitrateZones(qualityTarget(q, monitor(1280, 720, 60)));
    expect(small.recommended).toBeLessThan(bitrateZones(t(...FHD, 60)).recommended);
  });

  it("모니터보다 높은 주사율은 모니터 주사율로 줄여 계산한다", () => {
    const q = { width: 1920, height: 1080, fps: 144 };
    expect(bitrateZones(qualityTarget(q, monitor(1920, 1080, 60)))).toEqual(bitrateZones(t(...FHD, 60)));
    expect(bitrateZones(qualityTarget(q, monitor(1920, 1080, 165)))).toEqual(bitrateZones(t(...FHD, 144)));
  });

  it("원본 주사율은 모니터 주사율, 모르면 60", () => {
    const q = { width: 1920, height: 1080, fps: 0 };
    expect(bitrateZones(qualityTarget(q, monitor(1920, 1080, 144)))).toEqual(bitrateZones(t(...FHD, 144)));
    expect(bitrateZones(qualityTarget(q, monitor(1920, 1080, null)))).toEqual(bitrateZones(t(...FHD, 60)));
    expect(bitrateZones(qualityTarget(q, null))).toEqual(bitrateZones(t(...FHD, 60)));
  });
});

describe("zoneOf", () => {
  const z = { low: 3500, high: 17_500 };
  it("경계값은 적당", () => {
    expect(zoneOf(3499, z)).toBe("low");
    expect(zoneOf(3500, z)).toBe("ok");
    expect(zoneOf(17_500, z)).toBe("ok");
    expect(zoneOf(17_510, z)).toBe("high");
  });
});

describe("슬라이드 바(로그 눈금)", () => {
  const z = bitrateZones(t(...FHD, 60));
  it("양 끝과 바깥 값", () => {
    expect(sliderPos(z.barMin, z)).toBe(0);
    expect(sliderPos(z.barMax, z)).toBeCloseTo(SLIDER_STEPS);
    expect(sliderPos(1, z)).toBe(0);
    expect(sliderPos(1e9, z)).toBeCloseTo(SLIDER_STEPS);
  });
  it("구간 셋이 다 보인다: 부족·충분이 각각 바의 15% 이상, 적당이 가장 넓다", () => {
    const low = sliderPos(z.low, z) / SLIDER_STEPS;
    const high = sliderPos(z.high, z) / SLIDER_STEPS;
    expect(low).toBeGreaterThan(0.15);
    expect(1 - high).toBeGreaterThan(0.15);
    expect(high - low).toBeGreaterThan(Math.max(low, 1 - high));
  });
  it("위치 → 값은 10 Mbps 아래 0.1, 그 위 0.5 Mbps 단위이고 위치 ↔ 값이 거의 되돌아온다", () => {
    for (let p = 0; p <= SLIDER_STEPS; p += 5) {
      const kbps = sliderKbps(p, z);
      expect(kbps % (kbps < 10_000 ? 100 : 500)).toBe(0);
      expect(Math.abs(sliderPos(kbps, z) - p)).toBeLessThan(3);
    }
    expect(sliderKbps(Math.round(sliderPos(7000, z)), z)).toBeCloseTo(7000, -2);
  });
});

describe("글자·경고", () => {
  it("mbpsText", () => {
    expect(mbpsText(7000)).toBe("7");
    expect(mbpsText(17_500)).toBe("17.5");
    expect(mbpsText(6950)).toBe("6.95");
  });
  it("적당이면 경고 없음", () => {
    expect(bitrateWarning("ok", 7000, "lan")).toBeNull();
  });
  it("부족: 화면이 뭉개짐", () => {
    expect(bitrateWarning("low", 1500, "server")).toContain("뭉개");
  });
  it("충분 + 이 PC에서 호스팅: 업로드 = 비트레이트 × 시청자 수", () => {
    const text = bitrateWarning("high", 30_000, "direct");
    expect(text).toContain("업로드가 30 Mbps × 시청자 수");
    expect(text).not.toContain("요금");
    expect(bitrateWarning("high", 30_000, "lan", 3)).toContain("지금 3명이면 90 Mbps");
    expect(bitrateWarning("high", 30_000, "lan", 0)).not.toContain("지금");
  });
  it("충분 + 전용 서버: 시청자마다 내려받음", () => {
    const text = bitrateWarning("high", 25_000, "server");
    expect(text).toContain("시청자마다 25 Mbps");
    expect(text).not.toContain("업로드가");
  });
});
