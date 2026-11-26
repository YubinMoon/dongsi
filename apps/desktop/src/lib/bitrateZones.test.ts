import { describe, expect, it } from "vitest";
import {
  BAR_RATIO,
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
  it("기준 FHD 60fps H.264: 3.5 / 7 / 14.4 Mbps, 바 1.75~25 Mbps", () => {
    expect(bitrateZones(t(...FHD, 60))).toEqual({
      low: 3500,
      recommended: 7000,
      high: 14_400,
      barMin: 1750,
      barMax: 25_000,
    });
  });

  it("QHD 60fps: 바 끝이 최대 25 Mbps, 적당 5.4~17.4 Mbps", () => {
    expect(bitrateZones(t(...QHD, 60))).toEqual({
      low: 5400,
      recommended: 10_800,
      high: 17_400,
      barMin: 2700,
      barMax: 25_000,
    });
  });

  it("경계 순서: 부족 < 적당 < 충분 < 바 끝, 기본값은 적당 구간 안", () => {
    for (const size of [[1280, 720], [1720, 1000], FHD, QHD] as const) {
      for (const fps of [30, 60]) {
        const z = bitrateZones(t(size[0], size[1], fps));
        expect(z.barMin).toBeLessThan(z.low);
        expect(z.low).toBeLessThan(z.recommended);
        expect(z.recommended).toBeLessThan(z.high);
        expect(z.high).toBeLessThan(z.barMax);
        expect(zoneOf(z.recommended, z)).toBe("ok");
        expect(z.barMax).toBeLessThanOrEqual(25_000);
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

  it("충분 경계와 기본값은 YouTube 권장(일반 라이브)보다 낮다", () => {
    // https://support.google.com/youtube/answer/2853702 (2026-11-22 확인) H.264 권장 Mbps
    const youtube: [number, number, number, number][] = [
      [...FHD, 30, 14],
      [...FHD, 60, 17],
      [...QHD, 30, 21],
      [...QHD, 60, 34],
    ];
    for (const [w, h, fps, mbps] of youtube) {
      const z = bitrateZones(t(w, h, fps));
      expect(z.high).toBeLessThanOrEqual(mbps * 1000);
      expect(z.recommended).toBeLessThan(mbps * 1000 * 0.5);
    }
  });

  it("바 끝이 25 Mbps에 막히지 않으면 경계는 기본값의 0.5배·2.5배, 바 끝은 5배", () => {
    const z = bitrateZones(t(1280, 720, 30));
    expect(z.barMax).toBeLessThan(25_000);
    expect(z.low / z.recommended).toBeCloseTo(LOW_RATIO, 1);
    expect(z.high / z.recommended).toBeCloseTo(HIGH_RATIO, 1);
    expect(z.barMax / z.recommended).toBeCloseTo(BAR_RATIO, 1);
  });

  it("바 끝이 25 Mbps에 막히면 충분 경계는 기본값과 바 끝 사이(로그 눈금)의 같은 자리로 내려온다", () => {
    const at = Math.log(HIGH_RATIO) / Math.log(BAR_RATIO);
    for (const fps of [30, 60]) {
      for (const size of [FHD, QHD]) {
        const z = bitrateZones(t(size[0], size[1], fps));
        expect(z.barMax).toBe(25_000);
        expect(z.high / z.recommended).toBeLessThan(HIGH_RATIO);
        expect(Math.log(z.high / z.recommended) / Math.log(z.barMax / z.recommended)).toBeCloseTo(at, 1);
      }
    }
  });

  it("0.1 Mbps 단위로 반올림하고 허용 범위(0.3~25 Mbps) 안", () => {
    const tiny = bitrateZones(t(160, 90, 30));
    expect(tiny.low).toBe(300);
    expect(tiny.barMin).toBe(300);
    const huge = bitrateZones(t(7680, 4320, 240));
    expect(huge.high).toBe(25_000);
    expect(huge.barMax).toBe(25_000);
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
    const q = { width: 1920, height: 1080, fps: 60 };
    expect(bitrateZones(qualityTarget(q, monitor(1920, 1080, 30)))).toEqual(bitrateZones(t(...FHD, 30)));
    expect(bitrateZones(qualityTarget(q, monitor(1920, 1080, 165)))).toEqual(bitrateZones(t(...FHD, 60)));
  });

  it("30·60이 아닌 주사율(옛 원본 0, 144)은 60으로 계산한다(D-107)", () => {
    for (const fps of [0, 144]) {
      const q = { width: 1920, height: 1080, fps };
      expect(bitrateZones(qualityTarget(q, monitor(1920, 1080, 144)))).toEqual(bitrateZones(t(...FHD, 60)));
      expect(bitrateZones(qualityTarget(q, monitor(1920, 1080, null)))).toEqual(bitrateZones(t(...FHD, 60)));
      expect(bitrateZones(qualityTarget(q, null))).toEqual(bitrateZones(t(...FHD, 60)));
    }
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
  it("구간 셋이 다 보인다: 부족·충분이 각각 바의 15% 이상, 적당이 가장 넓다(FHD·QHD 30·60fps)", () => {
    for (const size of [FHD, QHD]) {
      for (const fps of [30, 60]) {
        const zz = bitrateZones(t(size[0], size[1], fps));
        const low = sliderPos(zz.low, zz) / SLIDER_STEPS;
        const high = sliderPos(zz.high, zz) / SLIDER_STEPS;
        expect(low).toBeGreaterThan(0.15);
        expect(1 - high).toBeGreaterThan(0.15);
        expect(high - low).toBeGreaterThan(Math.max(low, 1 - high));
      }
    }
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
    const text = bitrateWarning("high", 20_000, "direct");
    expect(text).toContain("업로드가 20 Mbps × 시청자 수");
    expect(text).not.toContain("요금");
    expect(bitrateWarning("high", 20_000, "lan", 3)).toContain("지금 3명이면 60 Mbps");
    expect(bitrateWarning("high", 20_000, "lan", 0)).not.toContain("지금");
  });
  it("충분 + 전용 서버: 시청자마다 내려받음", () => {
    const text = bitrateWarning("high", 25_000, "server");
    expect(text).toContain("시청자마다 25 Mbps");
    expect(text).not.toContain("업로드가");
  });
});
