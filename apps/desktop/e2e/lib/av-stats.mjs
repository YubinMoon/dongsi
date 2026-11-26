// av 시나리오의 순수 계산: Fisher 정확 검정, 첫 프레임 분해, 보정 지나침 지표, 팔·묶음별 집계.
// 하네스·브라우저와 무관한 함수만 둔다(시험: av-stats.test.mjs).

const mean = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);
const maxAbs = (xs) => (xs.length ? Math.max(...xs.map((x) => Math.abs(x))) : null);

/** 목록을 n칸 왼쪽으로 돌린다(rotate([on,off,stress], 1) → [off,stress,on]). 반복마다 팔 순서를 바꿔 시간 효과를 고르게 한다. */
export function rotate(list, n) {
  if (!list.length) return [];
  const k = ((n % list.length) + list.length) % list.length;
  return [...list.slice(k), ...list.slice(0, k)];
}

// ---------------------------------------------------------------------------------------------
// Fisher 정확 검정(2×2, 초기하분포)
// ---------------------------------------------------------------------------------------------
const logFacts = [0, 0];
function logFact(n) {
  for (let i = logFacts.length; i <= n; i++) logFacts[i] = logFacts[i - 1] + Math.log(i);
  return logFacts[n];
}
const logChoose = (n, k) => logFact(n) - logFact(k) - logFact(n - k);

/**
 * 두 묶음의 "사건이 있는 실행 수" 비교. 첫 묶음 n1번 중 k1번, 둘째 묶음 n2번 중 k2번.
 * 표 [[k1, n1−k1], [k2, n2−k2]]에서 행 합·열 합을 고정했을 때 첫 칸 x가 따르는 초기하분포를 쓴다.
 *  - pTwo     양측 p: P(x) ≤ P(관측) 인 모든 x의 확률 합(R fisher.test와 같은 규칙, 부동소수 오차 1e-7 허용)
 *  - pGreater 단측 p: 첫 묶음이 더 많다(x ≥ k1)
 *  - pLess    단측 p: 첫 묶음이 더 적다(x ≤ k1)
 */
export function fisherExact(k1, n1, k2, n2) {
  for (const v of [k1, n1, k2, n2]) if (!Number.isInteger(v) || v < 0) throw new Error("fisherExact: 0 이상의 정수여야 합니다");
  if (k1 > n1 || k2 > n2) throw new Error("fisherExact: 사건 수가 실행 수보다 많습니다");
  const N = n1 + n2;
  const K = k1 + k2;
  if (N === 0) return { pTwo: 1, pGreater: 1, pLess: 1 };
  const lo = Math.max(0, K - n2);
  const hi = Math.min(n1, K);
  const prob = (x) => Math.exp(logChoose(n1, x) + logChoose(n2, K - x) - logChoose(N, K));
  const pObs = prob(k1);
  let pTwo = 0;
  let pGreater = 0;
  let pLess = 0;
  for (let x = lo; x <= hi; x++) {
    const p = prob(x);
    if (p <= pObs * (1 + 1e-7)) pTwo += p;
    if (x >= k1) pGreater += p;
    if (x <= k1) pLess += p;
  }
  const c = (p) => Math.min(1, p);
  return { pTwo: c(pTwo), pGreater: c(pGreater), pLess: c(pLess) };
}

// ---------------------------------------------------------------------------------------------
// 첫 프레임 분해
// ---------------------------------------------------------------------------------------------
/**
 * 클릭 → 첫 프레임을 단계로 나눈다. 모두 Date.now()(같은 PC라 Node와 페이지의 시계가 같다).
 * @param pcs 페이지의 window.__t7pc 복사본([{ctorStart, created, offer, local, remote, connected}])
 * @param tClick 타일 클릭 직전 시각(Node Date.now())
 * @param firstWall 첫 프레임이 화면에 나온 시각(페이지 Date.now())
 * 클릭 뒤 처음 만든 PC 하나를 기준으로 한다. 없으면 { found: false, total }.
 * 단계(ms): clickToCreate(클릭→PC 생성 끝, 생성자 시간 ctorMs 포함), createToLocal, localToRemote(서버 왕복),
 * remoteToConnected, connectedToFrame, total. 시각이 비어 있으면 그 단계는 null.
 */
export function firstFrameBreakdown(pcs, tClick, firstWall) {
  const total = typeof firstWall === "number" && typeof tClick === "number" ? firstWall - tClick : null;
  const pc = (pcs ?? []).find((p) => typeof (p.ctorStart ?? p.created) === "number" && (p.ctorStart ?? p.created) >= tClick);
  if (!pc) return { found: false, total };
  const d = (a, b) => (typeof a === "number" && typeof b === "number" ? b - a : null);
  return {
    found: true,
    clickToCreate: d(tClick, pc.created),
    ctorMs: d(pc.ctorStart, pc.created),
    createToLocal: d(pc.created, pc.local),
    localToRemote: d(pc.local, pc.remote),
    remoteToConnected: d(pc.remote, pc.connected),
    connectedToFrame: d(pc.connected, firstWall),
    total,
  };
}

// ---------------------------------------------------------------------------------------------
// 보정 지나침: EPT 차이(E = 비디오 EPT − 오디오 EPT) 계열
// ---------------------------------------------------------------------------------------------
/**
 * @param ept analyze().series.ept ([{t(초), av(ms)}])
 * @param events detectDropouts().events ([{kind, t(초)}])
 * 반환:
 *  a: fromSec 뒤 최대|E|
 *  b: 끊김(av·audio) 사건 앞 beforeSec ~ 뒤 afterSec 구간을 뺀 fromSec 뒤 최대|E|
 *  c: 끊김(av·audio) 사건 뒤 afterSec 안(사건 t 초과, t+afterSec 이하)의 최소·최대 E(전체와 사건별)
 */
export function computeOvershoot(ept, events, { fromSec = 3, beforeSec = 0.5, afterSec = 12 } = {}) {
  const valid = (ept ?? []).filter((x) => Number.isFinite(x.av) && Number.isFinite(x.t));
  const late = valid.filter((x) => x.t >= fromSec);
  const cuts = (events ?? []).filter((e) => e.kind === "av" || e.kind === "audio").map((e) => ({ t: e.t, lo: e.t - beforeSec, hi: e.t + afterSec }));
  const outside = late.filter((x) => !cuts.some((c) => x.t >= c.lo && x.t <= c.hi));
  const perEvent = cuts.map((c) => {
    const vs = valid.filter((x) => x.t > c.t && x.t <= c.hi).map((x) => x.av);
    return { t: c.t, n: vs.length, min: vs.length ? Math.min(...vs) : null, max: vs.length ? Math.max(...vs) : null };
  });
  const mins = perEvent.filter((p) => p.n > 0).map((p) => p.min);
  const maxs = perEvent.filter((p) => p.n > 0).map((p) => p.max);
  return {
    a: maxAbs(late.map((x) => x.av)),
    b: maxAbs(outside.map((x) => x.av)),
    c: { events: cuts.length, min: mins.length ? Math.min(...mins) : null, max: maxs.length ? Math.max(...maxs) : null },
    perEvent,
  };
}

// ---------------------------------------------------------------------------------------------
// 집계: 대상 × 팔, 팔별 합계
// ---------------------------------------------------------------------------------------------
const hasDropouts = (r) => !!r.dropouts && !r.skipped && !r.error;

function aggregate(rs) {
  const evs = rs.flatMap((r) => (r.dropouts.events ?? []).map((e) => ({ ...e, rep: r.rep })));
  const cpu = rs.map((r) => r.cpu?.watchMean ?? r.cpu?.mean).filter((x) => Number.isFinite(x));
  return {
    runs: rs.length,
    av: evs.filter((e) => e.kind === "av").length,
    avRuns: rs.filter((r) => (r.dropouts.counts?.av ?? 0) > 0).length,
    videoOnly: evs.filter((e) => e.kind === "video").length,
    audioOnly: evs.filter((e) => e.kind === "audio").length,
    events: evs,
    cpuMean: mean(cpu),
    heavyRuns: rs.filter((r) => (r.heavy?.total ?? 0) > 0).length,
  };
}

/** 실행(runs[])을 대상·팔 순서(처음 나온 순서)로 묶어 집계한다. 끊김 분석이 없는 실행은 뺀다. */
export function groupRuns(runs) {
  const map = new Map();
  for (const r of runs.filter(hasDropouts)) {
    const key = [r.target, r.arm].join("|");
    if (!map.has(key)) map.set(key, { target: r.target, arm: r.arm, rs: [] });
    map.get(key).rs.push(r);
  }
  return [...map.values()].map((g) => ({ target: g.target, arm: g.arm, ...aggregate(g.rs) }));
}

/** 팔별 합계(전체 대상). 반환: [{arm, ...aggregate}] — 처음 나온 순서. */
export function armTotals(runs) {
  const map = new Map();
  for (const r of runs.filter(hasDropouts)) {
    if (!map.has(r.arm)) map.set(r.arm, []);
    map.get(r.arm).push(r);
  }
  return [...map.entries()].map(([arm, rs]) => ({ arm, ...aggregate(rs) }));
}

/** on 대 off, stress 대 off 의 Fisher 검정(동시 끊김이 있는 실행 수 기준). 두 팔이 모두 있을 때만. */
export function armTests(totals) {
  const by = Object.fromEntries(totals.map((t) => [t.arm, t]));
  const out = [];
  for (const [x, y] of [["on", "off"], ["stress", "off"]]) {
    if (!by[x] || !by[y] || by[x].runs === 0 || by[y].runs === 0) continue;
    out.push({ a: x, b: y, ka: by[x].avRuns, na: by[x].runs, kb: by[y].avRuns, nb: by[y].runs, ...fisherExact(by[x].avRuns, by[x].runs, by[y].avRuns, by[y].runs) });
  }
  return out;
}
