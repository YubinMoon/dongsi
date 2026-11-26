import { describe, expect, it } from "vitest";
import {
  addrSummary,
  fmt,
  formatAge,
  formatClock,
  initial,
  qualityLabel,
  secondsSince,
  viaLabel,
} from "./format";

describe("qualityLabel", () => {
  it("세로 해상도와 fps를 보인다", () => {
    expect(qualityLabel(1080, 60)).toBe("1080p · 60fps");
    expect(qualityLabel(720, 30)).toBe("720p · 30fps");
    expect(qualityLabel(1440, 60)).toBe("1440p · 60fps");
  });

  it("4K 이름은 따로 없다(최고 1440p, D-097): 큰 값도 세로 해상도 그대로", () => {
    expect(qualityLabel(2160, 30)).toBe("2160p · 30fps");
  });
});

describe("formatClock", () => {
  it("분:초와 시:분:초", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(5)).toBe("0:05");
    expect(formatClock(59)).toBe("0:59");
    expect(formatClock(60)).toBe("1:00");
    expect(formatClock(754)).toBe("12:34");
    expect(formatClock(3599)).toBe("59:59");
    expect(formatClock(3600)).toBe("1:00:00");
    expect(formatClock(3723)).toBe("1:02:03");
    expect(formatClock(36000)).toBe("10:00:00");
  });

  it("소수는 버리고 음수·NaN은 0", () => {
    expect(formatClock(5.9)).toBe("0:05");
    expect(formatClock(-3)).toBe("0:00");
    expect(formatClock(NaN)).toBe("0:00");
  });
});

describe("formatAge", () => {
  it("60초 미만은 방금", () => {
    expect(formatAge(0)).toBe("방금");
    expect(formatAge(59)).toBe("방금");
    expect(formatAge(-10)).toBe("방금");
    expect(formatAge(NaN)).toBe("방금");
  });

  it("분 단위", () => {
    expect(formatAge(60)).toBe("1분");
    expect(formatAge(119)).toBe("1분");
    expect(formatAge(1800)).toBe("30분");
    expect(formatAge(3599)).toBe("59분");
  });

  it("정시면 N시간, 아니면 N시간 M분", () => {
    expect(formatAge(3600)).toBe("1시간");
    expect(formatAge(3659)).toBe("1시간");
    expect(formatAge(3660)).toBe("1시간 1분");
    expect(formatAge(7200)).toBe("2시간");
    expect(formatAge(9000)).toBe("2시간 30분");
  });
});

describe("secondsSince", () => {
  it("지난 초를 센다", () => {
    expect(secondsSince(1000, 1_005_000)).toBe(5);
    expect(secondsSince(1000, 1_005_999)).toBe(5);
    expect(secondsSince(1000, 1_000_000)).toBe(0);
  });

  it("음수면 0", () => {
    expect(secondsSince(2000, 1_000_000)).toBe(0);
  });
});

describe("addrSummary", () => {
  it("개수에 따라 요약한다", () => {
    expect(addrSummary([])).toBe("");
    expect(addrSummary(["a"])).toBe("a");
    expect(addrSummary(["a", "b"])).toBe("a 외 1개");
    expect(addrSummary(["a", "b", "c"])).toBe("a 외 2개");
  });
});

describe("viaLabel", () => {
  it("경로 이름", () => {
    expect(viaLabel("lan")).toBe("LAN");
    expect(viaLabel("tailscale")).toBe("Tailscale");
    expect(viaLabel("local")).toBe("이 PC");
  });
});

describe("initial", () => {
  it("한글은 첫 글자", () => {
    expect(initial("준호")).toBe("준");
  });

  it("라틴은 대문자", () => {
    expect(initial("alice")).toBe("A");
    expect(initial("Bob")).toBe("B");
  });

  it("앞 공백은 무시한다", () => {
    expect(initial("  유나")).toBe("유");
  });

  it("빈 문자열·공백만 있으면 ?", () => {
    expect(initial("")).toBe("?");
    expect(initial("   ")).toBe("?");
  });

  it("서로게이트 쌍과 이모지를 깨뜨리지 않는다", () => {
    expect(initial("😀 방송")).toBe("😀");
    expect(initial("𠮷野家")).toBe("𠮷");
  });

  it("결합 문자와 ZWJ 이모지를 한 글자로 본다", () => {
    expect(initial("e\u{301}cole")).toBe("E\u{301}");
    expect(initial("👨‍👩‍👧 가족")).toBe("👨‍👩‍👧");
  });

  it("한글 자모 조합(NFD)도 한 글자", () => {
    const nfd = "한".normalize("NFD");
    expect(initial(nfd + "글")).toBe(nfd);
  });
});

describe("fmt", () => {
  it("null·undefined·NaN은 –", () => {
    expect(fmt(null)).toBe("–");
    expect(fmt(undefined)).toBe("–");
    expect(fmt(NaN)).toBe("–");
    expect(fmt(Infinity)).toBe("–");
  });

  it("자릿수를 맞춘다", () => {
    expect(fmt(12)).toBe("12");
    expect(fmt(12.5)).toBe("13");
    expect(fmt(12.345, 1)).toBe("12.3");
    expect(fmt(0, 2)).toBe("0.00");
  });

  it("0은 값으로 본다(–가 아니다)", () => {
    expect(fmt(0)).toBe("0");
  });

  it("-0 표시를 피한다", () => {
    expect(fmt(-0.0001)).toBe("0");
    expect(fmt(-0.0001, 2)).toBe("0.00");
    expect(fmt(-1.5)).toBe("-2");
  });
});
