// 검사 결과를 모아 results.json과 summary.md로 쓴다.
import fs from "node:fs";
import path from "node:path";

export const ALL_SHOTS = [
  "b-first-run", "b-home", "b-home-connecting", "b-home-connect-failed", "b-home-fingerprint",
  "b-server-menu", "b-rename", "b-remove", "b-home-toast", "b-add-server",
  "b-server", "b-server-empty", "b-server-disconnected",
  "b-settings",
  "b-watch", "b-watch-password", "b-watch-connecting", "b-watch-ended", "b-watch-error", "b-watch-popout",
  "b-broadcast-setup", "b-live", "b-live-notices",
];

const ICON = { pass: "통과", fail: "실패", warn: "경고", skip: "건너뜀", info: "참고" };

function fmtValue(v) {
  if (v === undefined || v === null) return "";
  if (typeof v === "string") return v;
  return JSON.stringify(v);
}

export class Recorder {
  constructor({ outDir, shotsDir, meta }) {
    this.outDir = outDir;
    this.shotsDir = shotsDir;
    this.meta = meta;
    this.entries = [];
    this.shots = new Map();
    this.measurements = [];
    this.scenario = "-";
    this.stepId = "-";
    this.stepStatus = new Map();
    this.t0 = Date.now();
  }

  setScenario(name) {
    this.scenario = name;
    this.stepId = "-";
    this.stepStatus = new Map();
  }

  add(status, name, value, detail) {
    const e = {
      scenario: this.scenario,
      step: this.stepId,
      name,
      status,
      value: value === undefined ? null : value,
      detail: detail ?? null,
      sec: Math.round((Date.now() - this.t0) / 100) / 10,
    };
    this.entries.push(e);
    const v = fmtValue(value);
    console.log(`  [${ICON[status]}] ${this.scenario}/${this.stepId} ${name}${v ? " — " + v.slice(0, 220) : ""}${detail ? " (" + String(detail).slice(0, 220) + ")" : ""}`);
    return e;
  }

  check(name, ok, value, detail) {
    return this.add(ok ? "pass" : "fail", name, value, detail);
  }
  /** cond가 참이면 경고(실패로 세지 않음), 아니면 통과 */
  warnIf(name, cond, value, detail) {
    return this.add(cond ? "warn" : "pass", name, value, detail);
  }
  info(name, value, detail) {
    return this.add("info", name, value, detail);
  }
  skip(name, reason) {
    return this.add("skip", name, null, reason);
  }
  fail(name, err) {
    return this.add("fail", name, null, String(err?.message ?? err).split("\n")[0]);
  }

  /**
   * 단계 하나. needs의 단계가 실패·건너뜀이면 실행하지 않고 건너뜀으로 적는다.
   * fn이 던지면 실패로 적고(onFail로 실패 화면을 찍는다) 다음 단계로 넘어간다.
   */
  async step(id, title, fn, { needs = [], onFail } = {}) {
    this.stepId = id;
    const bad = needs.filter((n) => this.stepStatus.get(n) !== "ok");
    console.log(`\n● ${this.scenario}/${id}: ${title}`);
    if (bad.length) {
      this.stepStatus.set(id, "skip");
      this.skip(title, "앞 단계(" + bad.join(", ") + ")가 성공하지 못해 건너뜀");
      return false;
    }
    try {
      await fn();
      this.stepStatus.set(id, "ok");
      return true;
    } catch (e) {
      this.stepStatus.set(id, "fail");
      this.fail(title, e);
      try {
        await onFail?.(id);
      } catch (e2) {
        console.log("  (실패 화면을 찍지 못함: " + String(e2.message ?? e2).split("\n")[0] + ")");
      }
      return false;
    }
  }

  registerShot(name, info) {
    this.shots.set(name, info);
  }

  addMeasurement(m) {
    this.measurements.push({ scenario: this.scenario, step: this.stepId, ...m });
  }

  counts() {
    const c = { pass: 0, fail: 0, warn: 0, skip: 0, info: 0 };
    for (const e of this.entries) c[e.status]++;
    return c;
  }

  get failed() {
    return this.entries.some((e) => e.status === "fail");
  }

  write(extra = {}) {
    const counts = this.counts();
    const shots = ALL_SHOTS.map((n) => {
      const s = this.shots.get(n);
      return s ? { name: n, taken: true, ...s } : { name: n, taken: false };
    });
    const results = {
      meta: { ...this.meta, durationSec: Math.round((Date.now() - this.t0) / 1000), ...extra },
      counts,
      entries: this.entries,
      measurements: this.measurements,
      shots,
    };
    fs.mkdirSync(this.outDir, { recursive: true });
    fs.writeFileSync(path.join(this.outDir, "results.json"), JSON.stringify(results, null, 2));
    fs.writeFileSync(path.join(this.outDir, "summary.md"), this.summary(results));
    return results;
  }

  summary(results) {
    const { counts, meta } = results;
    const L = [];
    L.push("# dongsi 실제 앱 통합 시험 결과", "");
    L.push(`- 시작: ${meta.startedAt} · 걸린 시간 ${meta.durationSec}초 · 시나리오: ${meta.scenarios.join(", ")}`);
    L.push(`- 커밋: ${meta.commit ?? "?"} · exe: ${meta.exe} (${meta.exeBuilt ?? "?"})`);
    L.push(`- **통과 ${counts.pass} · 실패 ${counts.fail} · 경고 ${counts.warn} · 건너뜀 ${counts.skip}** (참고 ${counts.info})`, "");
    const names = [...new Set(this.entries.map((e) => e.scenario))];
    for (const sc of names) {
      L.push(`## 시나리오 ${sc}`, "", "| 단계 | 검사 | 결과 | 값 / 설명 |", "|---|---|---|---|");
      for (const e of this.entries.filter((x) => x.scenario === sc)) {
        const v = [fmtValue(e.value), e.detail ? "(" + e.detail + ")" : ""].filter(Boolean).join(" ");
        L.push(`| ${e.step} | ${e.name.replace(/\|/g, "\\|")} | ${ICON[e.status]} | ${v.replace(/\|/g, "\\|").replace(/\n/g, " ").slice(0, 400)} |`);
      }
      L.push("");
    }
    if (results.measurements.length) {
      L.push("## 시청 지표(10초, 1초마다 측정)", "");
      L.push("A/V 차이 = 비디오 재생 시각 − 오디오 재생 시각(앱 통계 패널과 같은 부호, 양수면 비디오가 앞섬). 지연 = 지금(통계 시각) − 비디오 estimatedPlayoutTimestamp.", "");
      L.push("| 시나리오/단계 | 설명 | 해상도 | 코덱 | A/V 평균 | A/V |최대| | 지연 평균 | 지연 최대 | 지터버퍼 영상/소리 ms | 버린 프레임 | 프레임 수(3초) |", "|---|---|---|---|---|---|---|---|---|---|---|");
      const f = (x, d = 1) => (x === null || x === undefined ? "–" : Number(x).toFixed(d));
      for (const m of results.measurements) {
        L.push(`| ${m.scenario}/${m.step} | ${m.label} | ${m.width}×${m.height} | ${m.codec ?? "–"} | ${f(m.avMean)} ms | ${f(m.avMaxAbs)} ms | ${f(m.latMean)} ms | ${f(m.latMax)} ms | ${f(m.jbVideoMs)} / ${f(m.jbAudioMs)} | ${m.framesDropped} | ${m.framesIn3s} |`);
      }
      L.push("");
    }
    L.push("## 스크린샷", "", "| 이름 | 결과 | 크기 |", "|---|---|---|");
    for (const s of results.shots) {
      L.push(`| ${s.name} | ${s.taken ? "찍음" : "**못 찍음**(이번 실행에서 해당 시나리오를 돌리지 않았거나 단계가 실패)"} | ${s.size ? s.size.width + "×" + s.size.height : ""} |`);
    }
    L.push("");
    return L.join("\n");
  }
}
