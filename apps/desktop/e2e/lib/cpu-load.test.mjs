// cpu-load 순수 함수 시험. 실행: node --test e2e/lib/cpu-load.test.mjs
import assert from "node:assert/strict";
import test from "node:test";
import { parseTasklist, summarizeCpu } from "./cpu-load.mjs";

test("tasklist CSV 에서 빌드 프로세스만 센다(대소문자 무시)", () => {
  const text = [
    '"System Idle Process","0","Services","0","8 K"',
    '"rustc.exe","1234","Console","1","200,000 K"',
    '"RUSTC.EXE","1235","Console","1","180,000 K"',
    '"cargo.exe","1300","Console","1","30,000 K"',
    '"link.exe","1400","Console","1","90,000 K"',
    '"svchost.exe","900","Services","0","12,000 K"',
    '"lld-link.exe","1500","Console","1","10,000 K"', // 목록에 없는 이름은 세지 않는다
    "",
  ].join("\r\n");
  assert.deepEqual(parseTasklist(text), { rustc: 2, cargo: 1, link: 1, cl: 0, clang: 0, total: 4 });
  assert.deepEqual(parseTasklist(""), { rustc: 0, cargo: 0, link: 0, cl: 0, clang: 0, total: 0 });
});

test("CPU 요약: 전체와 시청 구간(watchFromWall 이후) 평균·최대", () => {
  const s = [{ wall: 1000, pct: 10 }, { wall: 2000, pct: 50 }, { wall: 3000, pct: 30 }];
  assert.deepEqual(summarizeCpu(s, 2000), { n: 3, mean: 30, max: 50, watchMean: 40, watchMax: 50 });
  assert.deepEqual(summarizeCpu([], 0), { n: 0, mean: null, max: null, watchMean: null, watchMax: null });
});
