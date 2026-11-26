// av-stats 순수 함수 시험. 실행: node --test e2e/lib/av-stats.test.mjs
import assert from "node:assert/strict";
import test from "node:test";
import { armTests, armTotals, computeOvershoot, firstFrameBreakdown, fisherExact, groupRuns, rotate } from "./av-stats.mjs";

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) < eps, a + " ≠ " + b);

test("팔 순서는 반복마다 한 칸씩 돈다", () => {
  const arms = ["off", "stress"];
  assert.deepEqual(rotate(arms, 0), ["off", "stress"]);
  assert.deepEqual(rotate(arms, 1), ["stress", "off"]);
  assert.deepEqual(rotate(arms, 2), ["off", "stress"]);
  assert.deepEqual(rotate(["a", "b", "c"], 2), ["c", "a", "b"]);
  assert.deepEqual(rotate(["off"], 5), ["off"]);
  assert.deepEqual(rotate([], 1), []);
  assert.deepEqual(arms, ["off", "stress"]); // 원본은 그대로
});

test("Fisher: 차 맛 구별 시험표 [[3,1],[1,3]] = 양측 34/70, 단측 17/70", () => {
  // 첫 칸 x 의 분포: C(4,x)·C(4,4−x)/70 = 1,16,36,16,1 (/70). 관측 x=3 (16/70)
  const r = fisherExact(3, 4, 1, 4);
  near(r.pTwo, 34 / 70); // P ≤ 16/70 인 x: 0,1,3,4
  near(r.pGreater, 17 / 70); // x ≥ 3
  near(r.pLess, 69 / 70); // x ≤ 3
});

test("Fisher: 극단 표 [[0,5],[5,0]] = 양측 2/252", () => {
  // x 의 분포: C(5,x)·C(5,5−x)/252 = 1,25,100,100,25,1 (/252). 관측 x=0
  const r = fisherExact(0, 5, 5, 5);
  near(r.pTwo, 2 / 252);
  near(r.pGreater, 1);
  near(r.pLess, 1 / 252);
});

test("Fisher: 위키백과 식이요법 표(남 1/10, 여 11/14) — 같은 확률 두 끝(3640)을 모두 더한다", () => {
  // N=24, K=12, C(24,12)=2704156. P(x)=C(10,x)·C(14,12−x)/2704156: x=0 → 91, x=1 → 3640, x=9 → 3640, x=10 → 91 (사이는 모두 3640 초과)
  const r = fisherExact(1, 10, 11, 14);
  near(r.pTwo, 7462 / 2704156, 1e-12);
  near(r.pLess, 3731 / 2704156, 1e-12);
  near(r.pGreater, 1 - 91 / 2704156, 1e-12);
  // 행을 바꾸면(여 11/14 대 남 1/10) 같은 양측 p, 단측은 방향이 뒤집힘
  const s = fisherExact(11, 14, 1, 10);
  near(s.pTwo, 7462 / 2704156, 1e-12);
  near(s.pGreater, 3731 / 2704156, 1e-12);
});

test("Fisher: 사건이 하나도 없으면 p = 1, 작은 표 [[2,0],[0,2]] 는 양측 1/3", () => {
  const z = fisherExact(0, 3, 0, 3);
  assert.deepEqual([z.pTwo, z.pGreater, z.pLess], [1, 1, 1]);
  // x ∈ {0,1,2}, P = 1,4,1 (/6). 관측 x=2
  const r = fisherExact(2, 2, 0, 2);
  near(r.pTwo, 2 / 6);
  near(r.pGreater, 1 / 6);
  assert.throws(() => fisherExact(3, 2, 0, 2));
});

test("첫 프레임 분해: 클릭 뒤 처음 만든 PC 기준, 단계 합 = 합계", () => {
  const pcs = [
    { ctorStart: 100, created: 110, offer: 120, local: 130, remote: 500, connected: 600 }, // 클릭 전(앞 실행)
    { ctorStart: 1300, created: 1650, offer: 1680, local: 1700, remote: 2600, connected: 2700 },
    { ctorStart: 5000, created: 5001, offer: null, local: null, remote: null, connected: null },
  ];
  const b = firstFrameBreakdown(pcs, 1000, 2950);
  assert.equal(b.found, true);
  assert.equal(b.clickToCreate, 650);
  assert.equal(b.ctorMs, 350);
  assert.equal(b.createToLocal, 50);
  assert.equal(b.localToRemote, 900);
  assert.equal(b.remoteToConnected, 100);
  assert.equal(b.connectedToFrame, 250);
  assert.equal(b.total, 1950);
  assert.equal(b.clickToCreate + b.createToLocal + b.localToRemote + b.remoteToConnected + b.connectedToFrame, b.total);
  // 클릭 뒤 PC 가 없으면 합계만
  assert.deepEqual(firstFrameBreakdown([pcs[0]], 1000, 2950), { found: false, total: 1950 });
  // connected 시각이 없으면 그 단계는 null
  const c = firstFrameBreakdown([{ ctorStart: 1100, created: 1200, local: 1250, remote: 1500, connected: null }], 1000, 1800);
  assert.equal(c.remoteToConnected, null);
  assert.equal(c.connectedToFrame, null);
  assert.equal(c.localToRemote, 250);
});

test("보정 지나침: 3초 뒤 최대|E|, 끊김 앞 0.5초~뒤 12초를 뺀 최대|E|, 끊김 뒤 12초의 최소·최대", () => {
  const ept = [
    { t: 1, av: 100 },
    { t: 2, av: -80 },
    { t: 3, av: 20 },
    { t: 4, av: -30 },
    { t: 5, av: 40 },
    { t: 6, av: 200 },
    { t: 7, av: 15 },
    { t: 30, av: 60 },
    { t: 31, av: -70 },
  ];
  const events = [
    { kind: "av", t: 5.5 },
    { kind: "video", t: 20 }, // video 만 끊긴 것은 제외 대상이 아니다
  ];
  const o = computeOvershoot(ept, events);
  assert.equal(o.a, 200); // t ≥ 3: 20,−30,40,200,15,60,−70
  assert.equal(o.b, 70); // [5.0, 17.5] 를 빼면 t=3(20), 4(−30), 30(60), 31(−70)
  assert.deepEqual(o.c, { events: 1, min: 15, max: 200 }); // (5.5, 17.5]: t=6(200), 7(15)
  assert.deepEqual(o.perEvent, [{ t: 5.5, n: 2, min: 15, max: 200 }]);
  // 사건이 없으면 a 와 b 가 같다
  const n = computeOvershoot(ept, []);
  assert.equal(n.a, 200);
  assert.equal(n.b, 200);
  assert.deepEqual(n.c, { events: 0, min: null, max: null });
});

const mkRun = (target, arm, rep, av, extra = {}) => ({
  target,
  arm,
  rep,
  cpu: { watchMean: 10, mean: 20 },
  heavy: { total: 0 },
  dropouts: { counts: { av, video: 0, audio: 0, total: av }, events: Array.from({ length: av }, () => ({ kind: "av" })) },
  ...extra,
});

test("집계: 대상·팔 묶음, 팔별 합계, stress 대 off 검정", () => {
  const runs = [
    mkRun("lan", "stress", 1, 1),
    mkRun("lan", "off", 1, 0),
    mkRun("lan", "stress", 2, 0, { heavy: { total: 2 }, cpu: { watchMean: 30 } }),
    mkRun("lan", "off", 2, 0),
    { target: "server", arm: "stress", rep: 1, skipped: "하드웨어 H.264 인코더 없음" }, // 분석 없음 → 제외
    { target: "server", arm: "off", rep: 1, error: "x", dropouts: { counts: { av: 9 }, events: [] } }, // 오류 → 제외
  ];
  const g = groupRuns(runs);
  assert.deepEqual(g.map((x) => [x.target, x.arm, x.runs, x.av, x.avRuns, x.heavyRuns]), [
    ["lan", "stress", 2, 1, 1, 1],
    ["lan", "off", 2, 0, 0, 0],
  ]);
  near(g[0].cpuMean, 20); // (10 + 30) / 2
  const tot = armTotals(runs);
  assert.deepEqual(tot.map((x) => [x.arm, x.runs, x.avRuns]), [["stress", 2, 1], ["off", 2, 0]]);
  const t = armTests(tot);
  assert.equal(t.length, 1);
  // [[1,1],[0,2]] → x ∈ {0,1}, P = 1/2, 1/2. 관측 x=1: 양측 1, 단측(stress 가 더 많음) 1/2
  assert.deepEqual([t[0].a, t[0].b, t[0].ka, t[0].na, t[0].kb, t[0].nb], ["stress", "off", 1, 2, 0, 2]);
  // off 팔만 있으면 검정하지 않는다
  assert.deepEqual(armTests(armTotals(runs.filter((r) => r.arm === "off"))), []);
  near(t[0].pTwo, 1);
  near(t[0].pGreater, 0.5);
});
