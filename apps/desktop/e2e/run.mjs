// 실제 앱 두 개를 CDP로 조작하는 통합 시험 하네스. 사용법은 README.md.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs, USAGE, VITE_PORT } from "./lib/args.mjs";
import { App } from "./lib/app.mjs";
import { killAllTrackedSync, killTree, spawnLogged, trackedPids } from "./lib/proc.mjs";
import { execFile } from "node:child_process";
import { Recorder } from "./lib/recorder.mjs";
import { sleep, tcpProbe, waitUntil } from "./lib/util.mjs";

const E2E = path.dirname(fileURLToPath(import.meta.url));
const DESKTOP = path.resolve(E2E, "..");
const REPO = path.resolve(DESKTOP, "../..");
const FFMPEG_BIN = process.env.DONGSI_FFMPEG_BIN ?? "D:\\dev\\ffmpeg\\bin";

const stamp = () => {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
};

function git(args) {
  try {
    return execFileSync("git", args, { cwd: REPO, encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

/** 포트를 듣고 있는 프로세스의 PID와 명령줄(Windows). 못 찾으면 null. */
function listenerOf(port) {
  return new Promise((resolve) => {
    execFile("netstat", ["-ano", "-p", "TCP"], { windowsHide: true, maxBuffer: 8 * 1024 * 1024 }, (err, out) => {
      if (err) return resolve(null);
      const m = out.split(/\r?\n/).find((l) => /LISTENING/.test(l) && new RegExp("[:\\]]" + port + "\\s").test(l));
      const pid = m ? Number(m.trim().split(/\s+/).pop()) : NaN;
      if (!Number.isInteger(pid)) return resolve(null);
      execFile(
        "powershell.exe",
        ["-NoProfile", "-Command", `(Get-CimInstance Win32_Process -Filter "ProcessId=${pid}").CommandLine`],
        { windowsHide: true },
        (e2, cmd) => resolve({ pid, cmd: e2 ? "" : String(cmd).trim() }),
      );
    });
  });
}

/**
 * 1420이 비어 있으면 vite dev 서버를 띄운다(끝나면 끈다). 이미 떠 있으면 그것을 쓰고 끄지 않는다.
 * 앱은 tauri.conf.json의 devUrl 때문에 1420만 불러온다. 떠 있는 vite가 이 폴더의 것이 아니면 다른 worktree의 화면 코드가 보이므로 경고한다.
 */
async function ensureVite(outDir) {
  const open = (await tcpProbe(VITE_PORT)) === "open";
  if (open) {
    const who = await listenerOf(VITE_PORT);
    const here = DESKTOP.toLowerCase().replace(/\//g, "\\");
    const mine = !!who?.cmd && who.cmd.toLowerCase().replace(/\//g, "\\").includes(here);
    let note;
    if (mine) note = `이미 떠 있음(이 폴더의 vite, pid ${who.pid}) — 끄지 않음`;
    else if (who?.cmd) note = `이미 떠 있음(pid ${who.pid}) — 경고: 이 폴더(${DESKTOP})가 아닌 곳에서 떴을 수 있어 화면 코드가 다를 수 있습니다. 명령줄: ${who.cmd.slice(0, 220)}`;
    else note = "이미 떠 있음(출처를 알 수 없음) — 경고: 다른 worktree의 화면 코드를 내줄 수 있습니다";
    console.log(`vite dev 서버(${VITE_PORT}): ${note}`);
    return { started: false, note, warn: !mine, stop: async () => {} };
  }
  const vite = path.join(DESKTOP, "node_modules", "vite", "bin", "vite.js");
  if (!fs.existsSync(vite)) throw new Error("node_modules가 없습니다. apps/desktop에서 npm install 하세요.");
  fs.mkdirSync(path.join(outDir, "logs"), { recursive: true });
  const proc = spawnLogged("vite", process.execPath, [vite, "dev"], { env: process.env, cwd: DESKTOP, logPrefix: path.join(outDir, "logs", "vite") });
  const note = `하네스가 띄움(pid ${proc.pid}, 끝나면 끔) — ${DESKTOP}`;
  console.log(`vite dev 서버(${VITE_PORT}): ${note}`);
  await waitUntil(async () => (await fetch(`http://127.0.0.1:${VITE_PORT}/`).catch(() => null))?.ok, { timeout: 60000, interval: 500, desc: "vite dev 서버" });
  return {
    started: true,
    note,
    warn: false,
    stop: async () => {
      await killTree(proc.pid);
    },
  };
}

async function main() {
  // exe 기본 위치: CARGO_TARGET_DIR가 있으면 그 아래, 없으면 저장소의 target (--exe로 직접 줄 수도 있다)
  const targetDir = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(REPO, "target");
  let args;
  try {
    args = parseArgs(process.argv.slice(2), { exe: path.join(targetDir, "debug", "dongsi-desktop.exe") });
  } catch (e) {
    // 앱·vite·Docker를 건드리기 전에 끝낸다.
    console.error("인자 오류: " + e.message + "\n(사용법: node e2e/run.mjs --help)");
    return 2;
  }
  if (args.help) {
    console.log(USAGE);
    return 0;
  }
  const cfg = args.cfg;
  const cfgLines = [
    `프로필(DONGSI_PROFILE): ${cfg.profiles.join(", ")}  [${cfg.sources.profiles}]`,
    `CDP 포트: ${cfg.cdp.join(", ")}  [${cfg.sources.cdp}]`,
    `포트 기준값 ${cfg.portBase}  [${cfg.sources.portBase}] → 앱 ${cfg.ports.app}, Docker 서버 ${cfg.ports.srv}, 응답 없는 TCP ${cfg.ports.hang}, 닫힌 포트 ${cfg.ports.closed}, 서버 카드용 ${cfg.ports.extra}`,
    `Docker 접두사: ${cfg.prefix}  [${cfg.sources.prefix}]`,
    `exe: ${args.exe}`,
    `vite: http://localhost:${VITE_PORT} (고정)`,
  ];
  console.log("이번 실행의 값:\n  " + cfgLines.join("\n  "));
  const outDir = args.out ?? path.join(E2E, "out", stamp());
  const shotsDir = args.shots ?? path.join(outDir, "shots");
  fs.mkdirSync(path.join(outDir, "logs"), { recursive: true });
  fs.mkdirSync(shotsDir, { recursive: true });

  const exeStat = fs.existsSync(args.exe) ? fs.statSync(args.exe) : null;
  const dirty = git(["status", "--porcelain", "--", "apps/desktop/src", "apps/desktop/src-tauri", "crates"]);
  const rec = new Recorder({
    outDir,
    shotsDir,
    meta: {
      startedAt: new Date().toISOString(),
      scenarios: args.scenario,
      commit: (git(["rev-parse", "--short", "HEAD"]) ?? "?") + (dirty ? " (+고친 파일 있음)" : ""),
      exe: args.exe,
      exeBuilt: exeStat ? exeStat.mtime.toISOString() : null,
      serverAddr: args.serverAddr,
      image: args.image,
      config: {
        profiles: cfg.profiles,
        cdp: cfg.cdp,
        portBase: cfg.portBase,
        ports: cfg.ports,
        prefix: cfg.prefix,
        vitePort: VITE_PORT,
        sources: cfg.sources,
      },
    },
  });
  console.log("결과 폴더: " + outDir);
  console.log("스크린샷 폴더: " + shotsDir);

  const onExit = () => killAllTrackedSync();
  process.on("exit", onExit);
  process.on("SIGINT", () => {
    onExit();
    process.exit(130);
  });

  let vite = null;
  try {
    if (!exeStat) throw new Error("앱 실행 파일이 없습니다: " + args.exe + "\n저장소 루트에서 cargo build -p dongsi-desktop 을 먼저 실행하세요(FFMPEG_DIR, LIBCLANG_PATH 필요).");
    vite = await ensureVite(outDir);
    rec.meta.config.vite = vite.note;
    rec.setScenario("setup");
    rec.add(vite.warn ? "warn" : "info", "vite dev 서버(1420)", vite.note);
    const ctx = {
      rec,
      args,
      outDir,
      shotsDir,
      repo: REPO,
      cfg,
      makeApp: (label, scenario) =>
        new App({
          label,
          profile: label === "A" ? cfg.profiles[0] : cfg.profiles[1],
          cdpPort: label === "A" ? cfg.cdp[0] : cfg.cdp[1],
          exe: args.exe,
          rec,
          outDir,
          shotsDir,
          ffmpegBin: FFMPEG_BIN,
          port: cfg.ports.app,
          scenario,
        }),
    };
    for (const name of args.scenario) {
      rec.setScenario(name);
      console.log(`\n========== 시나리오 ${name} ==========`);
      try {
        const mod = await import(`./scenarios/${name}.mjs`);
        await mod.run(ctx);
      } catch (e) {
        rec.fail("시나리오 " + name + " 전체", e);
        console.error(e);
      }
      await sleep(500);
    }
  } catch (e) {
    rec.setScenario("setup");
    rec.fail("준비", e);
    console.error(e);
  } finally {
    await vite?.stop();
    const left = trackedPids();
    if (left.length) console.log("남은 추적 프로세스 정리: " + left.map((x) => x.name + "#" + x.pid).join(", "));
    killAllTrackedSync();
  }
  const results = rec.write();
  console.log("\n===== 요약 =====");
  console.log(`통과 ${results.counts.pass} · 실패 ${results.counts.fail} · 경고 ${results.counts.warn} · 건너뜀 ${results.counts.skip}`);
  console.log("summary: " + path.join(outDir, "summary.md"));
  return rec.failed ? 1 : 0;
}

main().then(
  (code) => {
    process.exitCode = code;
  },
  (e) => {
    console.error("하네스 오류:", e);
    killAllTrackedSync();
    process.exitCode = 2;
  },
);
