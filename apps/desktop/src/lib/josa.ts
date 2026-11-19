/**
 * 한국어 조사 고르기. 이름 뒤에 붙는 조사를 마지막 글자의 받침으로 정한다.
 *
 * - 한글 음절: 받침 유무를 본다. "(으)로"는 받침이 없거나 ㄹ 받침이면 "로", 그 밖이면 "으로".
 * - 한글 자음 낱자(ㅋㅋ 등): 자음 이름(키읔 …)처럼 받침이 있다고 본다(ㄹ은 ㄹ 받침). 모음 낱자는 받침 없음.
 * - 숫자 0~9: 읽는 소리의 받침(영·일·삼·육·칠·팔은 받침 있음, 일·칠·팔은 ㄹ 받침).
 * - 알파벳: 글자 이름의 받침(L 엘·R 알은 ㄹ 받침, M 엠, N 엔). 그 밖의 글자는 받침 없음.
 *   단어 전체의 발음은 따지지 않는다(예: "Bob"은 "밥"으로 읽어도 B(비)로 본다).
 * - 끝의 공백·기호·이모지는 건너뛰고 그 앞 글자를 본다. 볼 글자가 없으면 받침 없음.
 */

export type JosaKind = "이/가" | "을/를" | "은/는" | "(으)로";

/** 종성 번호(0 = 없음, 8 = ㄹ). 한글 음절의 (코드 − 0xAC00) % 28 과 같은 번호를 쓴다. */
const JONG_NONE = 0;
const JONG_RIEUL = 8;

// 숫자 0~9를 읽을 때의 받침: 영(ㅇ) 일(ㄹ) 이 삼(ㅁ) 사 오 육(ㄱ) 칠(ㄹ) 팔(ㄹ) 구
const DIGIT_JONG = [21, 8, 0, 16, 0, 0, 1, 8, 8, 0];
// 알파벳 이름의 받침: L 엘, R 알(ㄹ), M 엠(ㅁ), N 엔(ㄴ)
const LETTER_JONG: Record<string, number> = { L: 8, R: 8, M: 16, N: 4 };

/** 마지막으로 읽히는 글자의 종성 번호. */
export function finalJong(text: string): number {
  const chars = Array.from(text);
  for (let i = chars.length - 1; i >= 0; i--) {
    const ch = chars[i];
    const code = ch.codePointAt(0) ?? 0;
    if (code >= 0xac00 && code <= 0xd7a3) return (code - 0xac00) % 28;
    // 호환용 한글 자모: ㄱ(0x3131)~ㅎ(0x314E) 자음, ㅏ(0x314F)~ㅣ(0x3163) 모음
    if (code >= 0x3131 && code <= 0x314e) return code === 0x3139 ? JONG_RIEUL : 1;
    if (code >= 0x314f && code <= 0x3163) return JONG_NONE;
    if (code >= 0x30 && code <= 0x39) return DIGIT_JONG[code - 0x30];
    if (/[A-Za-z]/.test(ch)) return LETTER_JONG[ch.toUpperCase()] ?? JONG_NONE;
    // 읽는 법을 알 수 없는 다른 글자(한자·키릴 …)는 받침 없음으로 둔다.
    if (/[\p{L}\p{N}]/u.test(ch)) return JONG_NONE;
    // 공백·기호·이모지는 건너뛴다.
  }
  return JONG_NONE;
}

/** 이름에 붙일 조사만 돌려준다. 예: josa("하린", "이/가") → "이", josa("서울", "(으)로") → "로". */
export function josa(name: string, kind: JosaKind): string {
  const jong = finalJong(name);
  const has = jong !== JONG_NONE;
  switch (kind) {
    case "이/가":
      return has ? "이" : "가";
    case "을/를":
      return has ? "을" : "를";
    case "은/는":
      return has ? "은" : "는";
    case "(으)로":
      return !has || jong === JONG_RIEUL ? "로" : "으로";
  }
}

/** 이름과 조사를 이어 붙인다. 예: withJosa("도윤", "이/가") → "도윤이". */
export function withJosa(name: string, kind: JosaKind): string {
  return name + josa(name, kind);
}
