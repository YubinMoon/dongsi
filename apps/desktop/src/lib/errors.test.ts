import { describe, expect, it } from "vitest";
import {
  closedReasonDetail,
  DEFAULT_CLOSED_REASON,
  errorCode,
  errorDetail,
  errorMessage,
  fingerprintMismatch,
  isMissingCommand,
  splitMessage,
} from "./errors";

const fp = (n: string) => ({ short: n.slice(0, 4), full: n });

describe("errorMessage / errorCode / errorDetail", () => {
  it("CmdError 모양", () => {
    const err = { message: "실패", code: "x", detail: { a: 1 } };
    expect(errorMessage(err)).toBe("실패");
    expect(errorCode(err)).toBe("x");
    expect(errorDetail(err)).toEqual({ a: 1 });
  });

  it("문자열·Error·null", () => {
    expect(errorMessage("그냥 문자열")).toBe("그냥 문자열");
    expect(errorMessage(new Error("boom"))).toBe("boom");
    expect(errorMessage(null)).toBe("null");
    expect(errorCode("문자열")).toBeNull();
    expect(errorCode(null)).toBeNull();
    expect(errorCode({ message: "m" })).toBeNull();
    expect(errorDetail({ message: "m" })).toBeUndefined();
    expect(errorDetail(null)).toBeUndefined();
  });
});

describe("fingerprintMismatch", () => {
  const stored = fp("aaaa1111");
  const actual = fp("bbbb2222");

  it("코드와 detail이 맞으면 두 지문을 돌려준다", () => {
    const err = { message: "인증서가 다릅니다", code: "fingerprintMismatch", detail: { stored, actual } };
    expect(fingerprintMismatch(err)).toEqual({ stored, actual });
  });

  it("다른 코드면 null", () => {
    expect(fingerprintMismatch({ message: "m", code: "passwordRequired", detail: { stored, actual } })).toBeNull();
    expect(fingerprintMismatch({ message: "m" })).toBeNull();
    expect(fingerprintMismatch("fingerprintMismatch")).toBeNull();
    expect(fingerprintMismatch(null)).toBeNull();
  });

  it("코드는 맞는데 detail이 없거나 모양이 틀리면 null", () => {
    expect(fingerprintMismatch({ message: "m", code: "fingerprintMismatch" })).toBeNull();
    expect(fingerprintMismatch({ message: "m", code: "fingerprintMismatch", detail: null })).toBeNull();
    expect(fingerprintMismatch({ message: "m", code: "fingerprintMismatch", detail: "x" })).toBeNull();
    expect(fingerprintMismatch({ message: "m", code: "fingerprintMismatch", detail: { stored } })).toBeNull();
    expect(
      fingerprintMismatch({ message: "m", code: "fingerprintMismatch", detail: { stored, actual: { short: "a" } } }),
    ).toBeNull();
    expect(
      fingerprintMismatch({ message: "m", code: "fingerprintMismatch", detail: { stored: "s", actual: "a" } }),
    ).toBeNull();
  });
});

describe("isMissingCommand", () => {
  it("문자열", () => {
    expect(isMissingCommand("Command disconnect_server not found")).toBe(true);
    expect(isMissingCommand("command connected_servers not found")).toBe(true);
  });

  it("객체", () => {
    expect(isMissingCommand({ message: "Command disconnect_server not found" })).toBe(true);
    expect(isMissingCommand(new Error("Command foo not found"))).toBe(true);
  });

  it("다른 오류는 아니다", () => {
    expect(isMissingCommand("서버에 연결하지 못했습니다")).toBe(false);
    expect(isMissingCommand({ message: "파일을 찾을 수 없습니다", code: "x" })).toBe(false);
    expect(isMissingCommand("file not found")).toBe(false);
    expect(isMissingCommand(null)).toBe(false);
  });
});

describe("splitMessage", () => {
  it("첫 문장과 나머지로 나눈다", () => {
    expect(splitMessage("미디어 연결에 실패했습니다. 방화벽에서 UDP가 막혀 있는지 확인해 주세요.")).toEqual({
      title: "미디어 연결에 실패했습니다.",
      detail: "방화벽에서 UDP가 막혀 있는지 확인해 주세요.",
    });
  });

  it("한 문장이면 detail은 null", () => {
    expect(splitMessage("서버에 연결하지 못했습니다.")).toEqual({ title: "서버에 연결하지 못했습니다.", detail: null });
    expect(splitMessage("마침표 없는 문장")).toEqual({ title: "마침표 없는 문장", detail: null });
  });

  it("숫자·주소 안의 점은 자르지 않는다", () => {
    expect(splitMessage("H.264 인코더가 없습니다.")).toEqual({ title: "H.264 인코더가 없습니다.", detail: null });
    expect(splitMessage("192.168.0.10:7884에 연결하지 못했습니다. 주소를 확인해 주세요.")).toEqual({
      title: "192.168.0.10:7884에 연결하지 못했습니다.",
      detail: "주소를 확인해 주세요.",
    });
  });

  it("셋 이상이면 나머지를 모두 설명에 넣는다", () => {
    expect(splitMessage("A입니다. B입니다. C입니다.")).toEqual({ title: "A입니다.", detail: "B입니다. C입니다." });
  });

  it("물음표·느낌표와 줄바꿈", () => {
    expect(splitMessage("정말요? 네.")).toEqual({ title: "정말요?", detail: "네." });
    expect(splitMessage("첫 줄.\n둘째 줄.")).toEqual({ title: "첫 줄.", detail: "둘째 줄." });
  });

  it("앞뒤 공백과 빈 문자열", () => {
    expect(splitMessage("  끝.  ")).toEqual({ title: "끝.", detail: null });
    expect(splitMessage("")).toEqual({ title: "", detail: null });
  });
});

describe("closedReasonDetail", () => {
  it("기본 문구면 null (끝의 마침표·공백 무시)", () => {
    expect(closedReasonDetail(DEFAULT_CLOSED_REASON)).toBeNull();
    expect(closedReasonDetail(DEFAULT_CLOSED_REASON + ".")).toBeNull();
    expect(closedReasonDetail(DEFAULT_CLOSED_REASON + ". ")).toBeNull();
    expect(closedReasonDetail("  " + DEFAULT_CLOSED_REASON + "  ")).toBeNull();
  });

  it("다른 이유는 그대로", () => {
    expect(closedReasonDetail("서버가 닫혔습니다.")).toBe("서버가 닫혔습니다.");
    expect(closedReasonDetail("연결을 끊었습니다")).toBe("연결을 끊었습니다");
  });

  it("비어 있으면 null", () => {
    expect(closedReasonDetail("")).toBeNull();
    expect(closedReasonDetail("  ")).toBeNull();
  });
});
