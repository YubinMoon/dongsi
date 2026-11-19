// 실행 중 CPU 부하(전체 %)와 같은 PC에서 빌드 프로세스가 돌고 있는지 재는 도우미.
// 측정이 같은 조건에서 이뤄졌는지 확인하려는 것이다(다른 작업자의 cargo 빌드 등이 끼어들면 끊김 비교가 깨진다).
import { execFile } from "node:child_process";
import os from "node:os";

const mean = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);

/** 모든 코어의 CPU 시간 합(밀리초 단위): { total, idle } */
export function cpuTimes() {
  let total = 0;
  let idle = 0;
  for (const c of os.cpus()) {
    const t = c.times;
    total += t.user + t.nice + t.sys + t.idle + t.irq;
    idle += t.idle;
  }
  return { total, idle };
}

/**
 * 표본 목록 [{wall, pct}]의 평균·최대. watchFromWall 이 있으면 wall ≥ 그 값인 표본만의 평균·최대도 낸다.
 * 반환: { n, mean, max, watchMean, watchMax } (표본이 없으면 null)
 */
export function summarizeCpu(samples, watchFromWall = null) {
  const all = samples.map((s) => s.pct);
  const watch = watchFromWall === null ? [] : samples.filter((s) => s.wall >= watchFromWall).map((s) => s.pct);
  return {
    n: samples.length,
    mean: mean(all),
    max: all.length ? Math.max(...all) : null,
    watchMean: mean(watch),
    watchMax: watch.length ? Math.max(...watch) : null,
  };
}

/**
 * 1초(intervalMs)마다 전체 CPU 사용률을 잰다. stop(watchFromWall)이 표본과 요약을 돌려준다.
 * 사용률 = 100 × (1 − idle 증가 ÷ 전체 증가), wall 은 그 구간이 끝난 시각(Date.now).
 */
export function startCpuMonitor({ intervalMs = 1000 } = {}) {
  let prev = cpuTimes();
  const samples = [];
  const timer = setInterval(() => {
    const cur = cpuTimes();
    const dt = cur.total - prev.total;
    const di = cur.idle - prev.idle;
    prev = cur;
    if (dt > 0) samples.push({ wall: Date.now(), pct: Math.round(1000 * (1 - di / dt)) / 10 });
  }, intervalMs);
  timer.unref?.();
  return {
    stop(watchFromWall = null) {
      clearInterval(timer);
      return { samples, ...summarizeCpu(samples, watchFromWall) };
    },
  };
}

const BUILD_NAMES = { "rustc.exe": "rustc", "cargo.exe": "cargo", "link.exe": "link", "cl.exe": "cl", "clang.exe": "clang" };

/** `tasklist /FO CSV /NH` 출력에서 빌드 프로세스 개수. 반환: { rustc, cargo, link, cl, clang, total } */
export function parseTasklist(text) {
  const out = { rustc: 0, cargo: 0, link: 0, cl: 0, clang: 0, total: 0 };
  for (const line of String(text).split(/\r?\n/)) {
    const m = /^"([^"]*)"/.exec(line.trim());
    const key = m ? BUILD_NAMES[m[1].toLowerCase()] : undefined;
    if (key) {
      out[key]++;
      out.total++;
    }
  }
  return out;
}

/** 지금 도는 빌드 프로세스 개수(Windows tasklist). 못 읽으면 null. */
export function countBuildProcs() {
  return new Promise((resolve) => {
    execFile("tasklist", ["/FO", "CSV", "/NH"], { windowsHide: true, timeout: 15000, maxBuffer: 1 << 24 }, (err, out) => {
      resolve(err ? null : parseTasklist(out));
    });
  });
}
