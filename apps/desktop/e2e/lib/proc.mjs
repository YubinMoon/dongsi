// 하네스가 띄운 프로세스만 추적해서 끈다. 다른 dongsi·vite 프로세스는 건드리지 않는다.
import { execFile, execFileSync, spawn } from "node:child_process";
import fs from "node:fs";

/** pid → 이름 */
const tracked = new Map();

export function spawnLogged(name, command, args, { env, cwd, logPrefix }) {
  const out = fs.openSync(logPrefix + ".stdout.log", "w");
  const err = fs.openSync(logPrefix + ".stderr.log", "w");
  const child = spawn(command, args, { env, cwd, stdio: ["ignore", out, err], windowsHide: false });
  fs.closeSync(out);
  fs.closeSync(err);
  child.on("exit", () => tracked.delete(child.pid));
  tracked.set(child.pid, name);
  const exited = new Promise((resolve) => child.once("exit", (code, sig) => resolve({ code, sig })));
  return { child, pid: child.pid, exited, name };
}

/** pid와 그 자식 전부를 끈다. 이미 없으면 조용히 넘어간다. */
export function killTree(pid) {
  return new Promise((resolve) => {
    if (!pid) return resolve();
    execFile("taskkill", ["/T", "/F", "/PID", String(pid)], { windowsHide: true }, () => {
      tracked.delete(pid);
      resolve();
    });
  });
}

/** 하네스가 띄운 것을 모두 끈다(비정상 종료·Ctrl+C 때). */
export function killAllTrackedSync() {
  for (const pid of [...tracked.keys()]) {
    try {
      execFileSync("taskkill", ["/T", "/F", "/PID", String(pid)], { windowsHide: true, stdio: "ignore" });
    } catch {
      // 이미 끝난 프로세스
    }
    tracked.delete(pid);
  }
}

export function trackedPids() {
  return [...tracked.entries()].map(([pid, name]) => ({ pid, name }));
}
