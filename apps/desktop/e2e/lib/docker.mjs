// WSL의 Docker로 리눅스 서버 컨테이너를 다룬다. 이름이 "t4-"로 시작하는 컨테이너·볼륨만 만들고 지운다.
import { execFile } from "node:child_process";
import { sleep } from "./util.mjs";

/** bash 스크립트를 WSL에서 돌린다. stdout·stderr를 따로 받는다(wsl.exe 출력이 한 파일에서 섞이지 않게). */
export function wsl(script, { timeout = 120000 } = {}) {
  return new Promise((resolve) => {
    execFile(
      "wsl.exe",
      ["-e", "bash", "-lc", script],
      { windowsHide: true, timeout, maxBuffer: 16 * 1024 * 1024, encoding: "buffer" },
      (err, stdout, stderr) => {
        const dec = (b) => b.toString("utf8").replace(/\x00/g, "").replace(/\r\n/g, "\n");
        resolve({ code: err ? (typeof err.code === "number" ? err.code : -1) : 0, out: dec(stdout), err: dec(stderr), timedOut: !!err?.killed });
      },
    );
  });
}

const q = (s) => "'" + String(s).replace(/'/g, "'\\''") + "'";
const assertT4 = (name) => {
  if (!/^t4-[a-z0-9-]+$/.test(name)) throw new Error("t4-로 시작하는 이름만 다룹니다: " + name);
};

export async function dockerReady(image) {
  const r = await wsl(`docker version --format '{{.Server.Version}}' && docker image inspect ${q(image)} --format '{{.Id}}'`, { timeout: 60000 });
  if (r.code !== 0) return { ok: false, reason: (r.err || r.out).trim().split("\n").slice(-2).join(" / ") || "wsl/docker를 쓸 수 없습니다" };
  return { ok: true, info: r.out.trim().split("\n").join(" · ") };
}

/** t4- 컨테이너·볼륨을 지운다(이름 t4-로 시작하는 것만). */
export async function cleanupT4() {
  const r = await wsl(
    `for c in $(docker ps -aq --filter 'name=^t4-'); do docker rm -f "$c" >/dev/null; done;
for v in $(docker volume ls -q --filter 'name=^t4-'); do docker volume rm "$v" >/dev/null; done; echo done`,
    { timeout: 90000 },
  );
  return r.code === 0;
}

/** 남아 있는 t4- 컨테이너·볼륨 이름 */
export async function listT4() {
  const r = await wsl(`docker ps -a --format '{{.Names}}' --filter 'name=^t4-'; echo ---; docker volume ls -q --filter 'name=^t4-'`, { timeout: 60000 });
  const [c, v] = r.out.split("---");
  const lines = (s) => (s ?? "").split("\n").map((x) => x.trim()).filter(Boolean);
  return { containers: lines(c), volumes: lines(v), ok: r.code === 0 };
}

const inviteFn = `invite_of() { docker logs "$1" 2>&1 | sed 's/\\x1b\\[[0-9;]*m//g' | grep -o 'dongsi1:[A-Za-z0-9_-]\\+' | tail -n 1; }`;

/**
 * 서버 컨테이너를 새로 띄우고 초대 코드를 돌려준다. 같은 이름의 컨테이너·볼륨이 있으면 먼저 지운다.
 * @param {{name:string, volume?:string, port:number, address:string, image:string}} o
 */
export async function startServer({ name, volume = name + "-data", port, address, image }) {
  assertT4(name);
  assertT4(volume);
  const script = `${inviteFn}
C=${q(name)}; V=${q(volume)}
docker rm -f "$C" >/dev/null 2>&1 || true
docker volume rm "$V" >/dev/null 2>&1 || true
docker run -d --name "$C" -p ${port}:${port}/tcp -p ${port}:${port}/udp -e DONGSI_PORT=${port} -e DONGSI_AUTO_ADDRESSES=false -e DONGSI_ADDRESSES=${q(address)} -e DONGSI_NAME=${q(name)} -v "$V":/data ${q(image)} >/dev/null || exit 1
for i in $(seq 1 300); do
  code="$(invite_of "$C")"
  if [ -n "$code" ]; then echo "$code"; exit 0; fi
  sleep 0.1
done
echo "30초 안에 초대 코드가 나오지 않았습니다" >&2
docker logs "$C" 2>&1 | tail -n 20 >&2
exit 1`;
  const r = await wsl(script, { timeout: 180000 });
  const m = r.out.match(/dongsi1:[A-Za-z0-9_-]+/g);
  if (r.code !== 0 || !m) throw new Error("서버 컨테이너를 시작하지 못했습니다: " + (r.err || r.out).trim().split("\n").slice(-6).join(" / "));
  return m[m.length - 1];
}

export async function dockerCmd(verb, name, { timeout = 60000 } = {}) {
  assertT4(name);
  if (!["stop", "start", "restart", "rm -f"].includes(verb)) throw new Error("허용하지 않는 명령: " + verb);
  const r = await wsl(`docker ${verb} ${verb === "stop" || verb === "restart" ? "-t 5 " : ""}${q(name)}`, { timeout });
  if (r.code !== 0) throw new Error(`docker ${verb} ${name} 실패: ${(r.err || r.out).trim()}`);
}

export async function logs(name, tail = 60) {
  assertT4(name);
  const r = await wsl(`docker logs --tail ${Number(tail)} ${q(name)} 2>&1 | sed 's/\\x1b\\[[0-9;]*m//g'`, { timeout: 30000 });
  return r.out;
}

/** 서버가 TCP를 받을 때까지 기다린다(docker start 뒤). */
export async function waitTcp(tcpProbe, port, timeout = 30000) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) {
    if ((await tcpProbe(port)) === "open") return true;
    await sleep(300);
  }
  return false;
}

// ---- 초대 코드 다루기 -------------------------------------------------------
const b64d = (s) => Buffer.from(s.replace(/-/g, "+").replace(/_/g, "/"), "base64").toString("utf8");
const b64e = (s) => Buffer.from(s, "utf8").toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

export function decodeInvite(code) {
  return JSON.parse(b64d(code.replace(/^dongsi\d+:/, "")));
}
/** 주소 후보를 바꾼 초대 코드. */
export function rewriteInvite(code, addrs) {
  const j = decodeInvite(code);
  j.a = addrs;
  return "dongsi1:" + b64e(JSON.stringify(j));
}
export function inviteInfo(code) {
  const j = decodeInvite(code);
  return { addrs: j.a, fingerprint: j.f, name: j.n ?? null };
}
