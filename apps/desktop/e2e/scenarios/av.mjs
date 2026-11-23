// 시나리오 av: A/V 차이·화면 전환 실측(--scenario av 로만 돈다. all 에는 들어 있지 않다).
//
// 앱 두 개(방송자 A, 시청자 B)에서 화면과 소리가 실제로 나온 시각을 재서, 대상(로컬 랜 / Docker 서버) × 코덱마다
//   - 시청 시작 뒤 구간별 A/V 차이(루프백·WebAudio·EPT 세 가지), 비디오·오디오 지연, 첫 프레임까지 시간
//   - 같은 시청 세션에서 화질·코덱을 바꿀 때의 적용 시간, 새 화면이 나오는 시간, 화면 공백, 회복 시간
// 을 표로 낸다. 측정 방법은 e2e/lib/sync-probe.mjs 머리말, 앱 쪽 도구는 src-tauri/src/dev_probe.rs.
// 화면 소스는 테스트 패턴, 소리는 테스트 톤만 쓴다(하네스 안전장치).
import { execFile } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { cleanupT4, dockerReady, inviteInfo, listT4, rewriteInvite, startServer } from "../lib/docker.mjs";
import {
  analyze,
  analyzeSwitch,
  detectDropouts,
  finishProbe,
  fmt,
  installProbe,
  primaryAv,
  readPublishRows,
  SEGMENTS,
  segName,
  startLoopback,
  stopLoopback,
  stopProbe,
  takeLoopback,
  wallToPerf,
} from "../lib/sync-probe.mjs";
import { AUDIBLE } from "../lib/app.mjs";
import { armTests, armTotals, computeOvershoot, firstFrameBreakdown, groupRuns, rotate } from "../lib/av-stats.mjs";
import { countBuildProcs, startCpuMonitor } from "../lib/cpu-load.mjs";
import { connectServerTab, liveTiles, openSetup, parseQuality, pickQuality, startBroadcastUi, stopBroadcastUi } from "../lib/ui.mjs";
import { primaryLanIp, sleep, tcpProbe } from "../lib/util.mjs";

/** 합격선(지금은 모두 경고로만 쓴다. 립싱크를 고친 뒤 실패로 바꾼다). 값은 여기 한곳에 둔다. */
export const LIMITS = {
  /** |A/V| 이 값 이하(ms) */
  avMs: 45,
  /** A/V 기준을 적용하기 시작하는 시청 시간(초): 립싱크 보정을 켠 팔(on) */
  avFromSec: 3,
  /** 같은 기준, 보정을 끈 팔(off·stress, 앱 기본 D-093): 브라우저 동기화는 약 20초에 걸쳐 맞춘다 */
  avFromSecBrowser: 20,
  /** 전환 뒤 회복(초) */
  recoverySec: 2,
  /** 전환 뒤 지표를 보는 구간(초) = 새 화면이 나와야 하는 시간 */
  switchWindowSec: 5,
  /** 전환 사이 간격(초) */
  switchGapSec: 7,
};

const CODEC_LABEL = { h264: "H.264", h264sw: "H.264", av1: "AV1" };
const LABEL_ID = { "H.264": "h264", AV1: "av1" };
const MIME = { h264: "video/H264", av1: "video/AV1" };
/** 전환 시험 순서(처음 것이 방송 시작 구성). 못 쓰는 코덱은 건너뛴다. */
const SWITCH_SEQ = [
  ["QHD 30fps", "H.264"],
  ["FHD 30fps", "AV1"],
  ["QHD 30fps", "H.264"],
  ["FHD 30fps", "AV1"],
  ["FHD 60fps", "H.264"],
  ["FHD 30fps", "AV1"],
  ["QHD 30fps", "H.264"],
  ["FHD 30fps", "AV1"],
  ["FHD 60fps", "AV1"],
];

/** pid 의 자손 프로세스(이름·pid·부모 pid). 프로세스 루프백(INCLUDE)이 WebView2 의 소리 프로세스를 포함하는지 확인하는 용도. 실패하면 null. */
function processTree(rootPid) {
  const ps = `$all = Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine; $ids = @(${rootPid}); $out = @(); $i = 0
while ($i -lt $ids.Count) { $cur = $ids[$i]; $i++; foreach ($p in ($all | Where-Object { $_.ParentProcessId -eq $cur })) { $ids += $p.ProcessId; $out += ($p.Name + '|' + $p.ProcessId + '|' + $p.ParentProcessId + '|' + $p.CommandLine) } }
$out -join [char]10`;
  return new Promise((resolve) => {
    execFile("powershell.exe", ["-NoProfile", "-Command", ps], { windowsHide: true, timeout: 20000, maxBuffer: 1 << 24 }, (err, out) => {
      if (err) return resolve(null);
      resolve(
        String(out)
          .split(/\r?\n/)
          .filter(Boolean)
          .map((line) => {
            const [name, pid, ppid, ...cmd] = line.split("|");
            const c = cmd.join("|");
            const type = /--type=([^\s"]+)/.exec(c)?.[1] ?? "";
            const sub = /--utility-sub-type=([^\s"]+)/.exec(c)?.[1] ?? "";
            return { name, pid: Number(pid), ppid: Number(ppid), kind: sub ? type + "/" + sub : type || "(browser)" };
          }),
      );
    });
  });
}

const treeText = (tree) => (tree ?? []).map((x) => x.name + ":" + x.pid + "<-" + x.ppid + "[" + x.kind + "]").join(" ");

const mean = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);

/**
 * 화질 이름("FHD 30fps"·"QHD 60fps", D-074·D-097·D-101) → 테스트 패턴의 출력 크기·fps와 방송 설정 값(want).
 * 테스트 패턴은 상자 안에 들어가는 가장 큰 16:9로 그리므로 FHD·QHD 상자면 상자 크기 그대로다
 * (crates/media `pattern_source_size`).
 */
function parsePreset(label) {
  const q = parseQuality(label);
  return { label: q.label, w: q.width, h: q.height, fps: q.fps, want: { w: q.width, h: q.height, fps: q.fps } };
}

// ---- 시나리오 b 의 서버 추가·연결 화면 조작(같은 동작을 둔다) ----
async function addServerUi(app, invite) {
  await app.go("#/");
  const page = await app.p();
  await page.locator('header nav button[aria-label="서버 추가"]').click();
  const dlg = page.locator("dialog[open]");
  await dlg.locator("#server-input").fill(invite);
  await dlg.locator(".preview").waitFor({ timeout: 10000 });
  await dlg.locator('button[type="submit"]').click();
  await page.locator("dialog[open]").waitFor({ state: "detached", timeout: 10000 });
}

/** 첫 서버 탭으로 가서 연결될 때까지 기다린다(홈에는 서버 카드가 없다, D-073). */
async function connectCard(app) {
  await connectServerTab(app);
}

/** 방송 시작 화면의 코덱 칩 상태 */
async function codecChip(app, label) {
  const page = await openSetup(app);
  const chips = await page.locator('.chips[aria-label="코덱"] .chip').evaluateAll((els) =>
    els.map((e) => ({ text: e.innerText.trim().replace(/\s+/g, " "), disabled: !!e.querySelector("input")?.disabled })),
  );
  const c = chips.find((x) => x.text === label || x.text.startsWith(label + " "));
  if (!c) return { ok: false, reason: "코덱 칩이 없음" };
  return c.disabled ? { ok: false, reason: c.text } : { ok: true };
}

const FIRST_FRAME_FN = () => {
  const v = document.querySelector("video.shown");
  if (v && v.videoWidth > 0 && v.readyState >= 2 && !v.paused) {
    if (!window.__sp_first) window.__sp_first = { perf: performance.now(), wall: Date.now() };
    return true;
  }
  return false;
};

/** A 의 화질 칩을 처음 누른 시각(Date.now, 칩을 누르면 바로 적용, D-074)과 방송 상태가 새 값이 된 시각을 페이지 안에서 잰다. */
const APPLY_HOOK = `(want) => {
  const my = { click: null, state: null, err: null };
  window.__sw = my;
  if (!window.__swL) {
    window.__swL = true;
    document.addEventListener("click", (e) => {
      const b = e.target && e.target.closest && e.target.closest(".chips .chip");
      if (b && window.__sw && window.__sw.click === null) window.__sw.click = Date.now();
    }, true);
  }
  (async () => {
    for (;;) {
      if (my.click !== null) {
        try {
          const st = await window.__t4invoke("broadcast_status");
          if (st && st.settings.codec === want.codec && st.settings.width === want.w && st.settings.height === want.h && st.settings.fps === want.fps) { my.state = Date.now(); return; }
        } catch (e) { my.err = String((e && e.message) || e); }
        if (Date.now() - my.click > 20000) return;
      }
      await new Promise((r) => setTimeout(r, 20));
    }
  })();
}`;

/** 팔 이름 → 시청 페이지 립싱크 보정을 끄는가. 보정은 기본으로 꺼져 있고(D-093) on 팔만 개발 빌드의 localStorage "dongsi.lipsync"="on"으로 켠다. stress 도 보정은 끈다. */
const LIPSYNC_OFF_ARM = { on: false, off: true, stress: true };

export async function run(ctx) {
  const { rec, args } = ctx;
  const cfg = args.av;
  const PORT = ctx.cfg.ports.srv;
  const PREFIX = ctx.cfg.prefix;
  const SRV = PREFIX + "srv";
  const A = ctx.makeApp("A", "av");
  const B = ctx.makeApp("B", "av");
  const failShots = (id) => Promise.all([A.failShot(id), B.failShot(id)]);
  const opts = (needs) => ({ needs, onFail: failShots });
  const runs = []; // 코덱별 결과
  const switchRuns = []; // 대상별 전환 결과
  const meta = {
    cfg,
    limits: LIMITS,
    arms: cfg.arms,
    repeat: cfg.repeat,
    // 옛 av.json 읽는 쪽 호환(팔이 하나일 때만 on/off, 여럿이면 mixed)
    lipsync: cfg.arms.length === 1 ? (LIPSYNC_OFF_ARM[cfg.arms[0]] ? "off" : "on") : "mixed",
    startedAt: new Date().toISOString(),
  };
  let encoders = [];
  let serverName = null;
  let serverId = null;
  let dockerUsed = false;
  let aEnvKey = null;
  let loopbackOpts = {}; // startLoopback 에 넘기는 값(자체 점검으로 정함)
  const preset = parsePreset(cfg.preset);
  const traceRoot = path.join(ctx.outDir, "trace");

  // 코덱 → 쓰이는 인코더(available_encoders 는 후보 순서대로라 코덱의 첫 항목이 실제로 쓰인다)
  const encoderFor = (codec) => {
    if (codec === "h264sw") {
      const sw = ["libopenh264"].find((n) => encoders.some((e) => e.codec === "h264" && e.name === n));
      return sw ? encoders.find((e) => e.name === sw) : null;
    }
    return encoders.find((e) => e.codec === LABEL_ID[CODEC_LABEL[codec]]) ?? null;
  };
  /** 송신 쪽 기록 폴더: <대상>-<이름>(전환 시험) 또는 <대상>-<코덱>-<팔>-r<반복>(코덱 실행마다 따로) */
  const traceDirOf = (target, name, arm = null, rep = null) => path.join(traceRoot, target + "-" + name + (arm ? "-" + arm + "-r" + rep : ""));
  const envFor = (target, name, codec, arm = null, rep = null) => {
    const env = {};
    if (codec === "h264sw") env.DONGSI_MEDIA_ENCODER = encoderFor("h264sw").name;
    if (cfg.trace) {
      fs.mkdirSync(traceRoot, { recursive: true });
      env.DONGSI_MEDIA_TRACE = traceDirOf(target, name, arm, rep);
    }
    return env;
  };
  const tagOf = (target, codec, arm, rep) => target + "/" + codec + "/" + arm + "#" + rep;
  /** stress 팔이면 probe 에 줄 jitterBufferTarget 변경 옵션 */
  const stressOpt = (arm) => (arm === "stress" ? { jbtStress: { periodMs: cfg.stressPeriod, highMs: cfg.stressMs } } : {});
  /** A 의 환경 변수가 바뀌면 A 를 다시 띄운다. */
  const ensureA = async (env) => {
    const key = JSON.stringify(env);
    if (key === aEnvKey) return;
    console.log("  A 다시 띄움(환경 변수 변경): " + key);
    await A.restart({ extraEnv: env });
    aEnvKey = key;
  };

  async function startWatch(target, arm) {
    const page = await B.p();
    await page.evaluate(() => {
      delete window.__sp_first;
    });
    const lb = AUDIBLE ? await startLoopback(B, loopbackOpts) : { skipped: true };
    let tile;
    if (target === "lan") {
      await B.go("#/");
      tile = liveTiles(page).first();
    } else {
      await B.go("#/server/" + serverId);
      tile = page.locator("ul.grid a.tile").first();
    }
    await tile.waitFor({ timeout: 25000 });
    // 시청 페이지 립싱크 보정 끔/켬(개발 빌드의 localStorage 스위치, 팔 비교용): on 만 켜고 off·stress 는 기본(끔, D-093) 그대로
    await page.evaluate((off) => {
      if (off) localStorage.removeItem("dongsi.lipsync");
      else localStorage.setItem("dongsi.lipsync", "on");
    }, LIPSYNC_OFF_ARM[arm] ?? true);
    const tClick = Date.now();
    await tile.click();
    await page.waitForFunction(() => location.hash.startsWith("#/watch/"), null, { timeout: 10000 });
    await page.waitForFunction(FIRST_FRAME_FN, null, { timeout: 40000, polling: 30 });
    const first = await page.evaluate(() => window.__sp_first);
    // 첫 프레임 분해용 PC 시각 기록(GUARD_SCRIPT 의 window.__t7pc)
    const pcs = await page.evaluate(() => JSON.parse(JSON.stringify(window.__t7pc || [])));
    return { page, lb, tClick, first, pcs };
  }

  /** B 에서 440Hz 를 0.7초 내고 루프백 최댓값을 본다(대상별로 소리가 잡히는지). */
  async function loopbackSanity(opts) {
    const page = await B.p();
    const st = await startLoopback(B, opts);
    if (st.error) return { opts, error: st.error, works: false, peak: 0 };
    await sleep(300);
    await page.evaluate(async () => {
      const ac = new AudioContext();
      const o = ac.createOscillator();
      const g = ac.createGain();
      g.gain.value = 0.3;
      o.connect(g).connect(ac.destination);
      o.start();
      await new Promise((r) => setTimeout(r, 700));
      o.stop();
      await ac.close();
    });
    await sleep(300);
    const tk = await takeLoopback(B);
    await stopLoopback(B);
    return { opts, samples: tk.samples, peak: tk.peak, works: (tk.peak ?? 0) > 0.05, error: tk.error ?? tk.lastError ?? null };
  }

  async function startBroadcast(target, label, presetLabel) {
    if (target === "server") await connectCard(A);
    return startBroadcastUi(A, { hosting: target === "lan" ? "lan" : "server", serverName, preset: presetLabel, codec: label });
  }

  async function endRun(w) {
    if (w) await stopProbe(w.page);
    await stopLoopback(B);
    await B.go("#/").catch(() => {});
    await stopBroadcastUi(A).catch(() => {});
  }

  // =============================================================================================
  async function codecRun(target, codec, arm, rep) {
    const tag = tagOf(target, codec, arm, rep);
    const label = CODEC_LABEL[codec];
    const enc = encoderFor(codec);
    const row = { target, codec, label, arm, rep, encoder: enc?.name ?? null, hardware: enc?.hardware ?? null };
    runs.push(row);
    if (!enc) {
      row.skipped = codec === "h264sw" ? "소프트웨어 H.264 인코더(libopenh264)가 없음" : "이 PC에 인코더가 없음";
      rec.skip(tag, row.skipped);
      return;
    }
    await ensureA(envFor(target, codec, codec, arm, rep));
    const chip = await codecChip(A, label);
    if (!chip.ok) {
      row.skipped = "코덱을 고를 수 없음: " + chip.reason;
      rec.skip(tag, row.skipped);
      return;
    }
    // 같은 PC 에서 빌드가 돌고 있었는지(한 번), 실행 동안 전체 CPU(1초마다)
    row.heavy = await countBuildProcs();
    if (row.heavy?.total) rec.add("warn", tag + ": 이 PC 에서 빌드 프로세스가 돌고 있음(측정이 흔들릴 수 있음)", JSON.stringify(row.heavy));
    const cpuMon = startCpuMonitor();
    let r;
    try {
      r = await startBroadcast(target, label, preset.label);
    } catch (e) {
      row.cpu = cpuMon.stop();
      throw e;
    }
    row.status = { width: r.status.width, height: r.status.height, settings: r.status.settings };
    rec.check(tag + ": 방송 상태의 코덱이 요청과 같음", r.status.settings.codec === LABEL_ID[label], JSON.stringify(r.status.settings));
    let w = null;
    let data = null;
    let an = null;
    try {
      w = await startWatch(target, arm);
      row.firstFrameMs = w.first.wall - w.tClick;
      row.firstFrame = firstFrameBreakdown(w.pcs, w.tClick, w.first.wall);
      row.loopbackStart = w.lb;
      const inst = await installProbe(w.page, { t0Perf: w.first.perf, statsMs: 500, ...stressOpt(arm) });
      if (inst.error) throw new Error("측정 스크립트 설치 실패: " + inst.error);
      rec.info(tag + ": 측정 설치", JSON.stringify({ ...inst, loopback: w.lb.skipped ? "끔(소리 안 냄)" : w.lb.error ? "오류: " + w.lb.error : "켜짐(pid " + w.lb.pid + ")", 첫프레임ms: row.firstFrameMs }));
      rec.info(tag + ": 첫 프레임 분해(ms)", JSON.stringify(row.firstFrame));
      const left = cfg.secs * 1000 - (Date.now() - w.first.wall);
      if (left > 0) await sleep(left);
      data = await finishProbe(w.page);
      const lb = AUDIBLE ? await takeLoopback(B) : { beeps: [], samples: 0, peak: 0, skipped: true };
      an = analyze(data, lb);
      if (an.error) throw new Error(an.error);
      row.analysis = { ...an, flashes: undefined };
      row.raw = { data, loopback: lb };
      const lastStat = [...data.stats].reverse().find((s) => s.v);
      row.decoder = lastStat?.v?.decoderImplementation ?? null;
      row.powerEfficientDecoder = lastStat?.v?.powerEfficientDecoder ?? null;
      row.mime = lastStat?.vMime ?? null;
      row.videoState = data.video;
      evaluateRun(tag, an, data, lb, arm);
    } catch (e) {
      row.error = String(e?.message ?? e);
      throw e;
    } finally {
      row.cpu = cpuMon.stop(w?.first?.wall ?? null);
      await endRun(w);
    }
    // 끊김·지나침 분석: 송신 쪽 기록(publish.csv)은 방송이 끝나 파일에 다 쓰인 뒤에 읽는다
    const traceDir = cfg.trace ? traceDirOf(target, codec, arm, rep) : null;
    const publish = traceDir ? readPublishRows(traceDir) : null;
    if (traceDir && !publish) rec.add("warn", tag + ": publish.csv 를 읽지 못함(송신 쪽 확인 없이 분석)", traceDir);
    row.dropouts = detectDropouts(data, { cpu: row.cpu.samples, publish });
    row.overshoot = computeOvershoot(an.series.ept, row.dropouts.events);
    row.lipsync = data.lipsync ?? null;
    row.jbt = data.jbt ?? [];
    const dc = row.dropouts.counts;
    rec.info(
      tag + ": 끊김",
      JSON.stringify({ 동시: dc.av, 비디오만: dc.video, 오디오만: dc.audio, CPU평균: row.cpu.watchMean === null ? null : +row.cpu.watchMean.toFixed(1), 빌드프로세스: row.heavy?.total ?? null }),
      dc.av > 0 ? "동시 끊김 t(s): " + row.dropouts.events.filter((e) => e.kind === "av").map((e) => fmt(e.t, 1)).join(", ") : undefined,
    );
  }

  function evaluateRun(tag, an, data, lb, arm) {
    const avFrom = (LIPSYNC_OFF_ARM[arm] ?? true) ? LIMITS.avFromSecBrowser : LIMITS.avFromSec;
    const c = an.counts;
    const needSeg = SEGMENTS.filter((s) => s[0] < cfg.secs - 1.5).length;
    const perSeg = (key) => an.segments.slice(0, needSeg).map((s) => (key === "flash" ? s.videoDelay.n : s[key].n));
    rec.info(
      tag + ": 개수",
      JSON.stringify({ 프레임: c.frames, 흰프레임: c.flashes, 삐루프백: c.loopbackBeeps, 삐WebAudio: c.webBeeps, 짝루프백: c.pairsLoopback, 짝WebAudio: c.pairsWeb, EPT: c.ept, 루프백샘플: c.loopbackSamples, 루프백최대: c.loopbackPeak }),
    );
    rec.info(tag + ": 시계 맞춤", JSON.stringify({ 왕복ms: +an.clock.rttMs.toFixed(2), 표류ms: an.clock.driftMs === null ? null : +an.clock.driftMs.toFixed(2), 오류: data.errors }));
    if (data.video?.muted) rec.add("warn", tag + ": 시청 video 가 음소거 상태(루프백에 소리가 안 나옴)", JSON.stringify(data.video));
    rec.check(tag + ": 흰 프레임이 구간마다 잡힘", perSeg("flash").every((n) => n >= 1), "구간별 " + perSeg("flash").join("/"));
    rec.check(tag + ": 삐가 WebAudio 로 구간마다 잡힘(흰 프레임과 짝)", perSeg("webaudio").every((n) => n >= 1), "구간별 " + perSeg("webaudio").join("/"));
    (AUDIBLE ? rec.check.bind(rec) : (name) => rec.skip(name, "소리 안 냄(--mute-audio)"))(
      tag + ": 삐가 루프백으로 구간마다 잡힘(흰 프레임과 짝)",
      perSeg("loopback").every((n) => n >= 1),
      "구간별 " + perSeg("loopback").join("/") + (lb.error ? " · 오류 " + lb.error : "") + (lb.lastError ? " · " + lb.lastError : ""),
    );
    const p = primaryAv(an);
    const late = p.series.filter((x) => x.t >= avFrom);
    const worst = late.length ? Math.max(...late.map((x) => Math.abs(x.av))) : null;
    rec.warnIf(
      tag + ": |A/V|(" + (p.key ?? "측정 없음") + " 기준) ≤ " + LIMITS.avMs + "ms (" + avFrom + "초 뒤부터)",
      worst === null || worst > LIMITS.avMs,
      worst === null ? "측정 없음" : "최대 |" + fmt(worst, 1) + "|ms, 평균 " + fmt(mean(late.map((x) => x.av)), 1, true) + "ms (" + late.length + "개)",
    );
  }

  // =============================================================================================
  async function switchRun(target) {
    const tag = target + "/전환";
    const row = { target, events: [] };
    switchRuns.push(row);
    await ensureA(envFor(target, "switch", "h264"));
    // 쓸 수 있는 구성만 (코덱 칩 확인)
    const usable = {};
    for (const [, c] of SWITCH_SEQ) if (!(c in usable)) usable[c] = (await codecChip(A, c)).ok;
    const seq = SWITCH_SEQ.filter(([, c]) => usable[c]);
    if (!seq.length || seq[0][1] !== SWITCH_SEQ[0][1]) {
      row.skipped = "시작 구성(H.264)을 고를 수 없음";
      rec.skip(tag, row.skipped);
      return;
    }
    const steps = [];
    for (let i = 0; i < cfg.switches; i++) steps.push(seq[(i + 1) % seq.length]);
    row.start = seq[0];
    const r = await startBroadcast(target, seq[0][1], seq[0][0]);
    row.status = { width: r.status.width, height: r.status.height, settings: r.status.settings };
    let w = null;
    try {
      w = await startWatch(target, cfg.arms[0]);
      row.arm = cfg.arms[0];
      row.firstFrameMs = w.first.wall - w.tClick;
      const inst = await installProbe(w.page, { t0Perf: w.first.perf, statsMs: 200, ...stressOpt(cfg.arms[0]) });
      if (inst.error) throw new Error("측정 스크립트 설치 실패: " + inst.error);
      await sleep(6000);
      const aPage = await A.p();
      if ((await A.hash()) !== "#/broadcast") await A.go("#/broadcast");
      await aPage.locator("h2", { hasText: "화질 변경" }).waitFor({ timeout: 10000 });
      let from = { ...parsePreset(seq[0][0]), codec: LABEL_ID[seq[0][1]] };
      const events = [];
      const t0 = Date.now();
      for (let i = 0; i < steps.length; i++) {
        const [pl, cl] = steps[i];
        const to = { ...parsePreset(pl), codec: LABEL_ID[cl] };
        to.mime = MIME[to.codec];
        const ev = { index: i + 1, from: { ...from }, to: { ...to }, label: from.label + " " + from.codec + " → " + pl + " " + to.codec, ok: false };
        try {
          await aPage.evaluate("(" + APPLY_HOOK + ")(" + JSON.stringify({ codec: to.codec, ...to.want }) + ")");
          // 바뀌는 칩만 누른다. 칩마다 바로 적용되고, 적용 중에 또 누르면 마지막 값만 이어서 보낸다.
          await pickQuality(aPage, pl, cl);
          await aPage.waitForFunction(() => window.__sw && window.__sw.state, null, { timeout: 20000, polling: 20 });
          const got = await aPage.evaluate(() => ({ click: window.__sw.click, state: window.__sw.state, err: window.__sw.err }));
          ev.clickWall = got.click;
          ev.stateWall = got.state;
          ev.ok = true;
        } catch (e) {
          ev.error = String(e?.message ?? e).split("\n")[0];
          rec.fail(tag + " " + ev.index + ": 화질 변경 적용", e);
        }
        events.push(ev);
        if (ev.ok) from = { ...to };
        const wait = t0 + (i + 1) * LIMITS.switchGapSec * 1000 - Date.now();
        if (wait > 0) await sleep(wait);
      }
      await sleep(Math.max(0, LIMITS.switchWindowSec * 1000 - 2000));
      const data = await finishProbe(w.page);
      const lb = AUDIBLE ? await takeLoopback(B) : { beeps: [], samples: 0, peak: 0, skipped: true };
      row.raw = { data, loopback: lb };
      row.errors = data.errors;
      row.events = events;
      rec.check(tag + ": 측정 기록이 남음(프레임·통계)", data.frames.length > 30 && data.stats.length > 5, data.frames.length + "프레임, 통계 " + data.stats.length + "개");
      const okEvents = events.filter((e) => e.ok);
      okEvents.forEach((ev, i) => {
        const next = okEvents[i + 1];
        ev.result = analyzeSwitch(data, { clickWall: ev.clickWall, stateWall: ev.stateWall, from: ev.from, to: ev.to }, {
          windowSec: LIMITS.switchWindowSec,
          nextClickPerf: next ? wallToPerf(data, next.clickWall) : null,
        });
        const x = ev.result;
        const t = tag + " " + ev.index + "(" + ev.label + ")";
        rec.info(
          t,
          JSON.stringify({ 상태반영ms: x.stateMs, 새화면ms: x.newMs === null ? null : Math.round(x.newMs), 새화면기준: x.newBy, 최대공백ms: x.maxGapMs === null ? null : Math.round(x.maxGapMs), 초당프레임: x.perSec, 키프레임: x.deltas.keyFrames, PLI: x.deltas.pli, freeze: x.deltas.freeze, 손실: x.deltas.lost, 회복ms: x.recoveryMs === null ? null : Math.round(x.recoveryMs) }),
        );
        const changed = ev.from.w !== ev.to.w || ev.from.h !== ev.to.h || ev.from.codec !== ev.to.codec;
        rec.warnIf(
          t + ": 새 화면이 " + LIMITS.switchWindowSec + "초 안에 나옴",
          changed && (x.newMs === null || x.newMs > LIMITS.switchWindowSec * 1000),
          x.newMs === null ? "나오지 않음" : Math.round(x.newMs) + "ms",
        );
        rec.warnIf(
          t + ": 회복 ≤ " + LIMITS.recoverySec + "초(표시 프레임 간격이 1초 동안 계속 2×프레임 간격 이하)",
          x.recoveryMs === null || x.recoveryMs > LIMITS.recoverySec * 1000,
          x.recoveryMs === null ? "회복하지 않음(다음 전환까지)" : Math.round(x.recoveryMs) + "ms",
        );
      });
      rec.check(tag + ": 요청한 전환 " + steps.length + "번이 모두 적용됨", okEvents.length === steps.length, okEvents.length + "/" + steps.length);
    } finally {
      await endRun(w);
    }
  }

  // =============================================================================================
  try {
    await rec.step("av1", "A·B 띄우고 인코더 목록 확인", async () => {
      A.resetState();
      B.resetState();
      await Promise.all([A.launch(), B.launch()]);
      aEnvKey = "{}";
      rec.check("안전장치 자체 시험 통과(A·B)", !!A.selfTestInfo?.ok && !!B.selfTestInfo?.ok);
      encoders = await A.invoke("available_encoders");
      meta.encoders = encoders;
      rec.info("쓸 수 있는 인코더", encoders.map((e) => e.name + (e.hardware ? "(하드웨어)" : "")).join(", "));
      const clk = await B.invoke("dev_clock", undefined, { raw: true });
      rec.check("개발용 명령 dev_clock 사용 가능(B)", clk.ok, clk.ok ? JSON.stringify(clk.v) : clk.e?.message);
      // 루프백 대상 정하기: 앱 프로세스 트리 → (안 되면) 오디오 서비스 프로세스 → (안 되면) 기본 출력 장치 전체
      if (!AUDIBLE) {
        meta.loopbackMode = "끔(하네스 기본 --mute-audio, 소리 안 냄)";
        loopbackOpts = {};
        rec.skip("루프백 점검", "하네스가 띄운 앱은 소리를 내지 않는다(실제 출력을 재려면 DONGSI_E2E_AUDIBLE=1)");
      } else {
        const tree = await processTree(B.proc.pid);
        meta.processTree = tree;
        rec.info("B 프로세스 트리(이름:pid<-부모[종류])", tree ? treeText(tree) : "조회 실패");
        const tries = [{ label: "앱 프로세스 트리(INCLUDE)", opts: {} }];
        // 오디오 서비스(utility/audio...)를 먼저, 나머지 자식은 그 뒤에
        const kids = [...(tree ?? [])].sort((a, b) => Number(/audio/i.test(b.kind)) - Number(/audio/i.test(a.kind)));
        for (const x of kids) tries.push({ label: "자식 프로세스 " + x.pid + " [" + x.kind + "](INCLUDE)", opts: { pid: x.pid } });
        tries.push({ label: "기본 출력 장치 전체(장치 루프백, 다른 소리도 섞임)", opts: { device: true } });
        meta.loopbackTries = [];
        loopbackOpts = null;
        for (const tr of tries) {
          const r = await loopbackSanity(tr.opts);
          meta.loopbackTries.push({ label: tr.label, ...r });
          rec.info("루프백 점검 — " + tr.label, JSON.stringify({ peak: r.peak, samples: r.samples, works: r.works, error: r.error }));
          if (r.works) {
            loopbackOpts = tr.opts;
            meta.loopbackMode = tr.label;
            break;
          }
        }
        rec.warnIf("루프백이 WebView2 소리를 잡는 대상을 찾음", loopbackOpts === null, meta.loopbackMode ?? "못 찾음(루프백 없이 WebAudio·EPT 만 측정)");
        loopbackOpts ??= {};
      }
      rec.info("측정 설정", JSON.stringify({ ...cfg, 한계: LIMITS }));
    }, opts([]));

    for (const target of cfg.targets) {
      const tid = "av-" + target;
      if (target === "server") {
        await rec.step(tid + "-srv", "WSL Docker 서버(" + SRV + ", 포트 " + PORT + ")를 띄우고 A·B 에 추가", async () => {
          const ready = await dockerReady(args.image);
          rec.check("WSL Docker 사용 가능, 이미지 " + args.image + " 있음", ready.ok, ready.ok ? ready.info : ready.reason);
          if (!ready.ok) throw new Error("Docker를 쓸 수 없습니다: " + ready.reason);
          dockerUsed = true;
          await cleanupT4(PREFIX);
          const host = args.serverAddr === "lan" ? await primaryLanIp() : "127.0.0.1";
          if (!host) throw new Error("이 PC의 LAN IPv4를 찾지 못했습니다(--server-addr lan)");
          const raw = await startServer({ name: SRV, port: PORT, address: host, image: args.image, prefix: PREFIX });
          const info = inviteInfo(raw);
          const want = host + ":" + PORT;
          const invite = info.addrs.includes(want) && info.addrs.length === 1 ? raw : rewriteInvite(raw, [want]);
          serverName = info.name ?? SRV;
          rec.check("서버 TCP " + PORT + " 접속 가능", (await tcpProbe(PORT)) === "open");
          await addServerUi(B, invite);
          await addServerUi(A, invite);
          serverId = (await B.invoke("list_servers"))[0]?.id;
          rec.check("A·B 에 서버가 저장됨", !!serverId && (await A.invoke("list_servers")).length >= 1, serverName);
          await connectCard(B);
        }, opts(["av1"]));
      }
      const needs = target === "server" ? ["av1", tid + "-srv"] : ["av1"];
      // 실행 순서: for 반복 { for 코덱(h264sw 제외) { for 팔(반복마다 한 칸씩 돌림) } }. 전환 시험은 대상마다 한 번(팔은 목록의 첫 값).
      // h264sw 는 A 를 다시 띄워야 하므로 맨 마지막에 같은 반복·팔 규칙으로 돈다.
      const runStep = (codec, arm, rep) =>
        rec.step(
          tid + "-" + codec + "-" + arm + "-r" + rep,
          `${target}: ${codec} 로 방송(팔 ${arm}, ${rep}/${cfg.repeat}번째) → B 가 새로 시청하며 ${cfg.secs}초 기록`,
          () => codecRun(target, codec, arm, rep),
          opts(needs),
        );
      for (let rep = 1; rep <= cfg.repeat; rep++) {
        for (const codec of cfg.codecs.filter((c) => c !== "h264sw")) {
          for (const arm of rotate(cfg.arms, rep - 1)) await runStep(codec, arm, rep);
        }
      }
      if (cfg.switches > 0) {
        await rec.step(tid + "-switch", `${target}: 같은 시청 세션에서 화질·코덱 ${cfg.switches}번 전환(팔 ${cfg.arms[0]})`, () => switchRun(target), opts(needs));
      }
      if (cfg.codecs.includes("h264sw")) {
        for (let rep = 1; rep <= cfg.repeat; rep++) {
          for (const arm of rotate(cfg.arms, rep - 1)) await runStep("h264sw", arm, rep);
        }
      }
    }
  } finally {
    await Promise.all([A.close(), B.close()]);
    if (dockerUsed) {
      if (!args.keep) {
        await cleanupT4(PREFIX);
        const left = await listT4(PREFIX);
        rec.check(PREFIX + " 컨테이너·볼륨이 정리됨", left.ok && left.containers.length === 0 && left.volumes.length === 0, JSON.stringify(left));
      } else {
        rec.info("--keep: " + PREFIX + " 컨테이너·볼륨을 남김", JSON.stringify(await listT4(PREFIX)));
      }
    }
    rec.addSection("A/V 실측: 코덱별 (구간 = 시청 시작(첫 프레임) 뒤 시간)", tableCodecs(runs));
    rec.addSection("끊김 비교(대상 × 코덱 × 팔)", tableDropoutCompare(runs));
    rec.addSection("끊김 목록", tableDropoutList(runs));
    rec.addSection("보정 지나침(EPT 차이 E)", tableOvershoot(runs));
    rec.addSection("첫 프레임 분해(클릭 → 첫 프레임, ms)", tableFirstFrame(runs));
    rec.addSection("화질·코덱 전환 실측", tableSwitches(switchRuns));
    try {
      fs.writeFileSync(path.join(ctx.outDir, "av.json"), JSON.stringify({ meta, runs, switches: switchRuns }));
      console.log("av.json: " + path.join(ctx.outDir, "av.json"));
    } catch (e) {
      console.log("av.json 을 쓰지 못함: " + e.message);
    }
  }
}

// =================================================================================================
// 표
// =================================================================================================
const cell = (m) => (!m || m.n === 0 ? "–" : fmt(m.mean, 0, true) + "/" + fmt(m.maxAbs, 0));
const meanSince = (series, key, from) => {
  const xs = series.filter((x) => x.t >= from).map((x) => x[key]);
  return xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null;
};

export function tableCodecs(runs) {
  const L = [];
  L.push("A/V = 소리 시각 − 화면 시각(양수 = 비디오가 앞섬, 앱 통계와 같은 부호). 구간 칸은 **평균/최대|값|(ms)**, L = 루프백(실제 출력), W = WebAudio, E = EPT 차이.");
  L.push("지연 = 정초(캡처) → 화면 표시/소리 출력(ms, 시청 10초 뒤 평균). W−L·E−L = 3초 뒤 A/V 평균의 차(경로별 측정 차이). 첫 프레임 = 시청 타일 클릭 → 첫 프레임(ms).", "");
  const head = ["대상", "코덱", "팔", "반복", "인코더 → 디코더", ...SEGMENTS.map((s) => segName(s)), "비디오 지연", "오디오 지연 L / W", "W−L", "E−L", "첫 프레임"];
  L.push("| " + head.join(" | ") + " |", "|" + head.map(() => "---").join("|") + "|");
  for (const r of runs) {
    const enc = r.encoder ? r.encoder + (r.hardware ? "(HW)" : "(SW)") + (r.decoder ? " → " + r.decoder : "") : "–";
    if (!r.analysis) {
      const note = r.skipped ? "건너뜀: " + r.skipped : r.error ? "오류: " + r.error.slice(0, 80) : "";
      L.push("| " + [r.target, r.codec, r.arm ?? "", r.rep ?? "", enc, note, ...SEGMENTS.slice(1).map(() => ""), "", "", "", "", ""].join(" | ") + " |");
      continue;
    }
    const an = r.analysis;
    const segCells = an.segments.map((s) => `L ${cell(s.loopback)} · W ${cell(s.webaudio)} · E ${cell(s.ept)}`);
    const vv = an.series.video.map((x) => ({ t: x.t, v: x.v }));
    const v10 = meanSince(vv, "v", 10) ?? meanSince(vv, "v", 0);
    const aL = meanSince(an.series.loopback, "aDelay", 10) ?? meanSince(an.series.loopback, "aDelay", 0);
    const aW = meanSince(an.series.webaudio, "aDelay", 10) ?? meanSince(an.series.webaudio, "aDelay", 0);
    const l3 = meanSince(an.series.loopback, "av", 3);
    const w3 = meanSince(an.series.webaudio, "av", 3);
    const e3 = meanSince(an.series.ept, "av", 3);
    const diff = (a, b) => (a === null || b === null ? null : a - b);
    L.push(
      "| " + [r.target, r.codec, r.arm ?? "", r.rep ?? "", enc, ...segCells, fmt(v10, 0), fmt(aL, 0) + " / " + fmt(aW, 0), fmt(diff(w3, l3), 0, true), fmt(diff(e3, l3), 0, true), r.firstFrameMs === undefined ? "–" : fmt(r.firstFrameMs, 0)].join(" | ") + " |",
    );
  }
  L.push("");
  return L;
}

export function tableSwitches(switchRuns) {
  const L = [];
  L.push("클릭 = 방송 화면의 화질 칩을 처음 누른 때(칩을 누르면 바로 적용, 해상도·주사율·코덱이 같이 바뀌면 여러 번 적용될 수 있다). 상태 = 방송 상태(broadcast_status)가 새 값이 된 때. 새 화면 = 새 해상도(또는 새 코덱 mimeType)의 첫 표시 프레임(코덱만 바뀌면 통계 주기 200ms 단위).");
  L.push("최대 공백 = 클릭 뒤 " + LIMITS.switchWindowSec + "초 안의 표시 프레임 사이 최대 간격. 프레임 수 = 클릭 뒤 1초 구간별 표시 프레임(0~" + LIMITS.switchWindowSec + "초). 회복 = 표시 프레임 간격이 처음으로 1초 동안 계속 (2 × 프레임 간격) 이하가 된 구간의 시작(새 화면이 나온 뒤부터).", "");
  const head = ["대상", "#", "변경", "상태 반영", "새 화면", "최대 공백", "프레임 수(1초별)", "키프레임", "PLI", "freeze", "손실", "회복"];
  L.push("| " + head.join(" | ") + " |", "|" + head.map(() => "---").join("|") + "|");
  for (const s of switchRuns) {
    if (s.skipped) {
      L.push("| " + [s.target, "–", "건너뜀: " + s.skipped, ...head.slice(3).map(() => "–")].join(" | ") + " |");
      continue;
    }
    for (const ev of s.events ?? []) {
      const x = ev.result;
      if (!x) {
        L.push("| " + [s.target, ev.index, ev.label, "적용 실패: " + (ev.error ?? "").slice(0, 60), ...head.slice(4).map(() => "–")].join(" | ") + " |");
        continue;
      }
      L.push(
        "| " + [s.target, ev.index, ev.label, fmt(x.stateMs, 0) + " ms", x.newMs === null ? "–" : fmt(x.newMs, 0) + " ms", x.maxGapMs === null ? "–" : fmt(x.maxGapMs, 0) + " ms", x.perSec.join(" / "), fmt(x.deltas.keyFrames, 0, true), fmt(x.deltas.pli, 0, true), fmt(x.deltas.freeze, 0, true), fmt(x.deltas.lost, 0, true), x.recoveryMs === null ? "**안 됨**" : fmt(x.recoveryMs, 0) + " ms"].join(" | ") + " |",
      );
    }
  }
  L.push("");
  return L;
}

// =================================================================================================
// 끊김·보정 지나침·첫 프레임 표
// =================================================================================================
const round0 = (x) => (x === null || x === undefined || Number.isNaN(x) ? "–" : String(Math.round(x)));
const fmtP = (p) => (p < 0.0001 ? "< 0.0001" : p.toFixed(4));
const okRuns = (runs) => runs.filter((r) => r.dropouts && !r.error && !r.skipped);

export function tableDropoutCompare(runs) {
  const L = [];
  L.push("끊김 = 화면과 소리가 함께(동시) 또는 따로 끊긴 사건. 비디오 공백 = rVFC 표시 시각 간격 ≥ 150ms, 오디오 은닉 = 500ms 통계 사이 concealedSamples 증가 ≥ 960(20ms). 두 구간을 각각 ±250ms 넓혀 겹치면 동시 끊김(av).");
  L.push("사건 길이는 동시 끊김마다 비디오 공백ms/오디오 은닉ms. CPU 평균 = 첫 프레임 뒤 시청 구간의 전체 CPU%(1초 표본). 빌드 = 실행 시작 때 rustc·cargo·link·cl·clang 프로세스가 하나라도 있던 실행 수.", "");
  const groups = groupRuns(runs);
  if (!groups.length) {
    L.push("(분석된 실행이 없음)", "");
    return L;
  }
  const head = ["대상", "코덱", "팔", "실행 수", "동시 끊김 사건", "동시 끊김 있는 실행", "사건 길이(비디오 ms/오디오 ms)", "비디오만", "오디오만", "CPU 평균 %", "빌드 있던 실행"];
  L.push("| " + head.join(" | ") + " |", "|" + head.map(() => "---").join("|") + "|");
  for (const g of groups) {
    const lens = g.events.filter((e) => e.kind === "av").map((e) => round0(e.videoGapMs) + "/" + round0(e.audioConcealMs));
    L.push("| " + [g.target, g.codec, g.arm, g.runs, g.av, g.avRuns, lens.length ? lens.join(", ") : "–", g.videoOnly, g.audioOnly, fmt(g.cpuMean, 0), g.heavyRuns].join(" | ") + " |");
  }
  L.push("");
  const tot = armTotals(runs);
  L.push("**팔별 합계**(모든 대상·코덱)", "");
  const th = ["팔", "실행 수", "동시 끊김 사건", "동시 끊김 있는 실행", "있는 실행 비율", "비디오만", "오디오만", "CPU 평균 %", "빌드 있던 실행"];
  L.push("| " + th.join(" | ") + " |", "|" + th.map(() => "---").join("|") + "|");
  for (const t of tot) {
    L.push("| " + [t.arm, t.runs, t.av, t.avRuns, fmt((100 * t.avRuns) / t.runs, 0) + "%", t.videoOnly, t.audioOnly, fmt(t.cpuMean, 0), t.heavyRuns].join(" | ") + " |");
  }
  L.push("");
  const tests = armTests(tot);
  if (tests.length) {
    L.push("**Fisher 정확 검정**(동시 끊김이 있는 실행 수 기준, 2×2 초기하분포. 단측 = 앞 팔이 더 많음)", "");
    for (const t of tests) {
      L.push(`- ${t.a} 대 ${t.b}: ${t.ka}/${t.na} 대 ${t.kb}/${t.nb} — 양측 p = ${fmtP(t.pTwo)}, 단측 p(${t.a} > ${t.b}) = ${fmtP(t.pGreater)}`);
    }
    L.push("", "실행이 적으면 검정력이 낮다(같은 PC·같은 시각 조건이라도 사건이 드물면 p 가 커진다). 같은 대상·코덱을 반복해 실행 수를 늘려 본다.", "");
  } else {
    L.push("(Fisher 검정은 on·off 또는 stress·off 팔이 함께 있을 때만 낸다)", "");
  }
  return L;
}

export function tableDropoutList(runs) {
  const L = [];
  L.push("t = 첫 프레임 뒤 초(비디오는 공백 시작, 오디오만은 은닉 구간 가운데). 보정값 = 그 시각 립싱크 adjustMs. 바닥 변경 뒤 = 마지막 applied 뒤 ms. jbt 변경 뒤 = stress 팔의 마지막 변경 뒤 ms.");
  L.push("NetEq = 사건 끝 뒤 3초 안 오디오 jitterBufferTargetDelay / jitterBufferDelay 구간 평균(ms)의 최댓값. 송신 = --av-trace 일 때 사건 시각 [−1.5s, +0.5s] 안 publish.csv 의 큐 지연 최댓값, 같은 종류(v/a) 캡처 간격 최댓값(ms).", "");
  const head = ["대상", "코덱", "팔", "반복", "종류", "t(s)", "비디오 공백 ms", "오디오 은닉 ms", "보정값 ms", "바닥 변경 뒤 ms", "jbt 변경 뒤 ms", "NetEq 목표/지연 ms", "CPU %", "송신 큐 지연 최대 ms", "송신 캡처 간격 최대 ms"];
  const rows = [];
  for (const r of okRuns(runs)) {
    for (const e of r.dropouts.events) {
      const kind = e.kind === "av" ? "동시" : e.kind === "video" ? "비디오만" : "오디오만";
      const sd = e.send;
      const lag = sd ? fmt(e.kind === "video" ? sd.lagMaxMs.v : e.kind === "audio" ? sd.lagMaxMs.a : sd.lagMaxMs.all, 1) : "–";
      const gap = sd ? (e.kind === "video" ? fmt(sd.capGapMaxMs.v, 1) : e.kind === "audio" ? fmt(sd.capGapMaxMs.a, 1) : "v " + fmt(sd.capGapMaxMs.v, 1) + " / a " + fmt(sd.capGapMaxMs.a, 1)) : "–";
      rows.push([r.target, r.codec, r.arm, r.rep, kind, fmt(e.t, 1), round0(e.videoGapMs), round0(e.audioConcealMs), fmt(e.adjustMs, 1), round0(e.sinceFloorChangeMs), round0(e.sinceJbtMs), round0(e.neteqTargetMs) + " / " + round0(e.neteqDelayMs), fmt(e.cpuPct, 0), lag, gap]);
    }
  }
  if (!rows.length) {
    L.push("(끊김 없음)", "");
    return L;
  }
  L.push("| " + head.join(" | ") + " |", "|" + head.map(() => "---").join("|") + "|");
  for (const x of rows) L.push("| " + x.join(" | ") + " |");
  L.push("");
  return L;
}

export function tableOvershoot(runs) {
  const L = [];
  L.push("E = 비디오 EPT − 오디오 EPT(ms, 양수 = 비디오가 앞섬). 3초 뒤 최대|E| = 시청 3초부터. 끊김 뺀 = 끊김(동시·오디오) 사건 앞 0.5초 ~ 뒤 12초를 뺀 3초 뒤 최대|E|. 끊김 뒤 12초 = 사건 뒤 12초 안의 E 최소·최대(사건이 여럿이면 전체).", "");
  const head = ["대상", "코덱", "팔", "반복", "3초 뒤 최대 |E|", "끊김 뺀 3초 뒤 최대 |E|", "끊김 뒤 12초 E 최소", "끊김 뒤 12초 E 최대", "끊김(동시·오디오) 수"];
  const rows = okRuns(runs).filter((r) => r.overshoot);
  if (!rows.length) {
    L.push("(자료 없음)", "");
    return L;
  }
  L.push("| " + head.join(" | ") + " |", "|" + head.map(() => "---").join("|") + "|");
  for (const r of rows) {
    const o = r.overshoot;
    L.push("| " + [r.target, r.codec, r.arm, r.rep, fmt(o.a, 0), fmt(o.b, 0), fmt(o.c.min, 0, true), fmt(o.c.max, 0, true), o.c.events].join(" | ") + " |");
  }
  L.push("");
  return L;
}

export function tableFirstFrame(runs) {
  const L = [];
  L.push("클릭 = 시청 타일 클릭. 클릭 뒤 처음 만든 RTCPeerConnection 기준(페이지 Date.now). 괄호 안 생성자 = new RTCPeerConnection() 호출 시간(첫 PC 는 WebView2 에서 수백 ms). local→remote 에는 서버 연결·시청 요청·answer 왕복이 들어 있다.", "");
  const head = ["대상", "코덱", "팔", "반복", "클릭 → PC 생성(생성자)", "PC 생성 → local 설정", "local → remote(answer 적용)", "remote → connected", "connected → 첫 프레임", "합계"];
  const rows = runs.filter((r) => r.firstFrame && !r.skipped);
  if (!rows.length) {
    L.push("(자료 없음)", "");
    return L;
  }
  L.push("| " + head.join(" | ") + " |", "|" + head.map(() => "---").join("|") + "|");
  for (const r of rows) {
    const f = r.firstFrame;
    if (!f.found) {
      L.push("| " + [r.target, r.codec, r.arm, r.rep, "PC 를 찾지 못함", "–", "–", "–", "–", round0(f.total)].join(" | ") + " |");
      continue;
    }
    L.push("| " + [r.target, r.codec, r.arm, r.rep, round0(f.clickToCreate) + " (" + round0(f.ctorMs) + ")", round0(f.createToLocal), round0(f.localToRemote), round0(f.remoteToConnected), round0(f.connectedToFrame), round0(f.total)].join(" | ") + " |");
  }
  L.push("");
  return L;
}
