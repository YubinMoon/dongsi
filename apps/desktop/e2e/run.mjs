// 실제 앱 두 개를 CDP로 조작하는 통합 시험 하네스. 사용법은 README.md.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs, USAGE } from "./lib/args.mjs";
import { App } from "./lib/app.mjs";
import { killAllTrackedSync, killTree, spawnLogged, trackedPids } from "./lib/proc.mjs";
import { Recorder } from "./lib/recorder.mjs";
import { sleep, tcpProbe, waitUntil } from "./lib/util.mjs";

const E2E = path.dirname(fileURLToPath(import.meta.url));
const DESKTOP = path.resolve(E2E, "..");
const REPO = path.resolve(DESKTOP, "../..");
const FFMPEG_BIN = process.env.DONGSI_FFMPEG_BIN ?? "D:\\dev\\ffmpeg\\bin";
const VITE_PORT = 1420;

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

/** 1420이 비어 있으면 vite dev 서버를 띄운다. 이미 떠 있으면 그것을 쓴다. */
async function ensureVite(outDir) {
  const open = (await tcpProbe(VITE_PORT)) === "open";
  if (open) {
    console.log(`vite dev 서버가 이미 ${VITE_PORT}에서 돌고 있어 그대로 씁니다(끄지 않음).`);
    return { started: false, stop: async () => {} };
  }
  const vite = path.join(DESKTOP, "node_modules", "vite", "bin", "vite.js");
  if (!fs.existsSync(vite)) throw new Error("node_modules가 없습니다. apps/desktop에서 npm install 하세요.");
  fs.mkdirSync(path.join(outDir, "logs"), { recursive: true });
  const proc = spawnLogged("vite", process.execPath, [vite, "dev"], { env: process.env, cwd: DESKTOP, logPrefix: path.join(outDir, "logs", "vite") });
  console.log(`vite dev 서버를 띄웁니다(pid ${proc.pid}, 포트 ${VITE_PORT}).`);
  await waitUntil(async () => (await fetch(`http://127.0.0.1:${VITE_PORT}/`).catch(() => null))?.ok, { timeout: 60000, interval: 500, desc: "vite dev 서버" });
  return {
    started: true,
    stop: async () => {
      await killTree(proc.pid);
    },
  };
}

async function main() {
  const args = parseArgs(process.argv.slice(2), { exe: path.join(REPO, "target", "debug", "dongsi-desktop.exe") });
  if (args.help) {
    console.log(USAGE);
    return 0;
  }
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
    const ctx = {
      rec,
      args,
      outDir,
      shotsDir,
      repo: REPO,
      makeApp: (label, scenario) =>
        new App({
          label,
          profile: label === "A" ? "t4a" : "t4b",
          cdpPort: label === "A" ? 9225 : 9226,
          exe: args.exe,
          rec,
          outDir,
          shotsDir,
          ffmpegBin: FFMPEG_BIN,
          port: 8450,
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
