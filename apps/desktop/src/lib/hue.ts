/** 같은 문자열은 늘 같은 색조(0~359)를 돌려준다. FNV-1a 32비트 해시를 360으로 나눈 나머지. */
export function hueFor(seed: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < seed.length; i++) {
    h ^= seed.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h % 360;
}

/**
 * 타일 색조를 CSS 변수로 넘기는 인라인 style 문자열(`--h: 70`).
 * 색은 CSS에서 `oklch(0.25 0.035 var(--h))` 같은 식으로 계산한다. 다른 style과 이으려면 `;`을 직접 붙인다.
 */
export function tileVars(h: number): string {
  return `--h: ${h}`;
}
