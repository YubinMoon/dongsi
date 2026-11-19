import { describe, expect, it } from "vitest";
import { finalJong, josa, withJosa } from "./josa";

describe("josa 이/가", () => {
  it("받침이 없으면 가, 있으면 이", () => {
    expect(josa("하린", "이/가")).toBe("이");
    expect(josa("민수", "이/가")).toBe("가");
    expect(josa("도윤", "이/가")).toBe("이");
    expect(josa("우리집 서버", "이/가")).toBe("가");
  });
});

describe("josa 을/를, 은/는", () => {
  it("받침에 따라 고른다", () => {
    expect(josa("하린", "을/를")).toBe("을");
    expect(josa("민수", "을/를")).toBe("를");
    expect(josa("하린", "은/는")).toBe("은");
    expect(josa("민수", "은/는")).toBe("는");
  });
});

describe("josa (으)로", () => {
  it("받침 없음 → 로", () => {
    expect(josa("민수", "(으)로")).toBe("로");
  });
  it("ㄹ 받침 → 로", () => {
    expect(josa("서울", "(으)로")).toBe("로");
    expect(josa("하늘", "(으)로")).toBe("로");
  });
  it("그 밖의 받침 → 으로", () => {
    expect(josa("하린", "(으)로")).toBe("으로");
    expect(josa("집", "(으)로")).toBe("으로");
    expect(josa("밖", "(으)로")).toBe("으로");
  });
});

describe("한글이 아닌 끝 글자", () => {
  it("숫자는 읽는 소리의 받침을 따른다", () => {
    expect(josa("서버1", "이/가")).toBe("이"); // 일
    expect(josa("서버2", "이/가")).toBe("가"); // 이
    expect(josa("서버3", "이/가")).toBe("이"); // 삼
    expect(josa("서버0", "이/가")).toBe("이"); // 영
    expect(josa("서버9", "이/가")).toBe("가"); // 구
    expect(josa("서버1", "(으)로")).toBe("로"); // 일은 ㄹ 받침
    expect(josa("서버3", "(으)로")).toBe("으로"); // 삼
    expect(josa("서버7", "(으)로")).toBe("로"); // 칠
  });
  it("알파벳은 글자 이름의 받침을 따른다", () => {
    expect(josa("PC", "이/가")).toBe("가"); // 씨
    expect(josa("MN", "이/가")).toBe("이"); // 엔
    expect(josa("L", "(으)로")).toBe("로"); // 엘
    expect(josa("M", "(으)로")).toBe("으로"); // 엠
    expect(josa("pc", "을/를")).toBe("를");
  });
  it("끝의 공백·기호·이모지는 건너뛴다", () => {
    expect(josa("하린 ", "이/가")).toBe("이");
    expect(josa("하린!", "이/가")).toBe("이");
    expect(josa("민수)", "이/가")).toBe("가");
    expect(josa("하린🎮", "이/가")).toBe("이");
  });
  it("볼 글자가 없으면 받침 없음", () => {
    expect(josa("", "이/가")).toBe("가");
    expect(josa("   ", "(으)로")).toBe("로");
    expect(josa("🎮", "은/는")).toBe("는");
  });
  it("한글 자모 낱자와 읽는 법을 모르는 글자", () => {
    expect(josa("ㅋㅋ", "이/가")).toBe("이");
    expect(josa("ㅏ", "이/가")).toBe("가");
    expect(josa("ㄹ", "(으)로")).toBe("로");
    expect(josa("東", "이/가")).toBe("가");
  });
});

describe("withJosa·finalJong", () => {
  it("이름과 조사를 잇는다", () => {
    expect(withJosa("하린", "이/가")).toBe("하린이");
    expect(withJosa("민수", "이/가")).toBe("민수가");
  });
  it("종성 번호", () => {
    expect(finalJong("서울")).toBe(8);
    expect(finalJong("민수")).toBe(0);
  });
});
