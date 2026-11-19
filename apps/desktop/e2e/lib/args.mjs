// 명령줄 인자(> 환경 변수 > 기본값)와 값 검사
import path from "node:path";

export const USAGE = `사용법: node e2e/run.mjs --scenario a|b|shots|all [옵션]

  --scenario <a|b|shots|all>   돌릴 시나리오(기본 all). 쉼표로 여럿: a,shots
  --out <폴더>                 결과 폴더(기본 e2e/out/<시각>/)
  --shots <폴더>               스크린샷 폴더(기본 <out>/shots)
  --image <이름>               시나리오 b의 서버 Docker 이미지(기본 dongsi-server:e2e)
  --server-addr <loopback|lan> 시나리오 b의 서버 주소: 127.0.0.1 또는 이 PC의 LAN IPv4(기본 loopback)
  --keep                       끝난 뒤 Docker 컨테이너·볼륨(접두사로 시작하는 것)을 지우지 않는다
  --exe <경로>                 앱 실행 파일(기본 <CARGO_TARGET_DIR 또는 저장소 target>/debug/dongsi-desktop.exe)

다른 작업과 겹치지 않게 하는 값(인자 > 환경 변수 > 기본값):
  --profiles <a>,<b>           앱 두 개의 DONGSI_PROFILE        (DONGSI_E2E_PROFILES, 기본 t4a,t4b)
  --cdp <a>,<b>                앱 두 개의 CDP 포트              (DONGSI_E2E_CDP, 기본 9225,9226)
  --port-base <n>              포트 기준값 n                    (DONGSI_E2E_PORT_BASE, 기본 8400)
                               앱 DONGSI_PORT = n+50, Docker 서버 = n+60, 응답 없는 TCP = n+70,
                               아무도 안 듣는 포트 = n+71, 서버 카드용 = n+72
  --prefix <p>                 Docker 컨테이너·볼륨 이름 접두사 (DONGSI_E2E_PREFIX, 기본 t4-)
  --help
`;

export const DEFAULTS = { profiles: "t4a,t4b", cdp: "9225,9226", portBase: "8400", prefix: "t4-" };
export const VITE_PORT = 1420;
const DEFAULT_PORT = 7884;

const PROFILE_RE = /^[A-Za-z0-9_-]{1,64}$/;
const PREFIX_RE = /^[a-z0-9][a-z0-9_.-]*-$/;

const pair = (label, v) => {
  const parts = String(v).split(",").map((x) => x.trim());
  if (parts.length !== 2 || parts.some((x) => x === "")) throw new Error(`${label}는 쉼표로 구분한 두 값이어야 합니다: "${v}"`);
  return parts;
};

const intIn = (label, v, lo, hi) => {
  const n = typeof v === "number" ? v : /^\d+$/.test(String(v).trim()) ? Number(String(v).trim()) : NaN;
  if (!Number.isInteger(n) || n < lo || n > hi) throw new Error(`${label}는 ${lo}~${hi} 사이의 정수여야 합니다: "${v}"`);
  return n;
};

/** 프로필·CDP·포트·접두사를 검사해 계산한 설정을 돌려준다. 잘못되면 던진다(앱을 띄우기 전). */
export function resolveConfig({ profiles, cdp, portBase, prefix }) {
  const profs = pair("--profiles", profiles);
  for (const p of profs) {
    if (!PROFILE_RE.test(p)) throw new Error(`프로필 이름은 ^[A-Za-z0-9_-]{1,64}$ 꼴이어야 합니다: "${p}"`);
  }
  if (profs[0] === profs[1]) throw new Error("프로필 두 개는 서로 달라야 합니다: " + profs[0]);
  const cdps = pair("--cdp", cdp).map((x) => intIn("CDP 포트", x, 1024, 65535));
  if (cdps[0] === cdps[1]) throw new Error("CDP 포트 두 개는 서로 달라야 합니다: " + cdps[0]);
  const base = intIn("--port-base", portBase, 0, 65535);
  if (!PREFIX_RE.test(prefix)) throw new Error(`--prefix는 ^[a-z0-9][a-z0-9_.-]*-$ 꼴이어야 합니다(예: t4-): "${prefix}"`);
  const ports = { app: base + 50, srv: base + 60, hang: base + 70, closed: base + 71, extra: base + 72 };
  const seen = new Map();
  for (const [name, port] of Object.entries(ports)) {
    if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error(`포트 기준값 ${base}에서 계산한 ${name} 포트(${port})가 1024~65535를 벗어납니다`);
    if (port === DEFAULT_PORT) throw new Error(`포트 기준값 ${base}에서 계산한 ${name} 포트가 ${DEFAULT_PORT}(기본 포트, 사람이 쓰는 포트)입니다. 다른 기준값을 쓰세요`);
    seen.set(port, name);
  }
  for (const c of cdps) {
    if (seen.has(c)) throw new Error(`CDP 포트 ${c}가 계산한 ${seen.get(c)} 포트와 겹칩니다`);
    seen.set(c, "cdp");
  }
  if (seen.has(VITE_PORT)) throw new Error(`포트 ${VITE_PORT}은 vite dev 서버(고정)가 쓰므로 ${seen.get(VITE_PORT)} 포트로 쓸 수 없습니다`);
  return { profiles: profs, cdp: cdps, portBase: base, prefix, ports };
}

export function parseArgs(argv, defaults, env = process.env) {
  const args = {
    scenario: ["a", "b", "shots"],
    out: null,
    shots: null,
    image: "dongsi-server:e2e",
    serverAddr: "loopback",
    keep: false,
    exe: defaults.exe,
    help: false,
  };
  // 값 출처 기록(시작할 때 출력용): 인자 > 환경 변수 > 기본값
  const raw = {
    profiles: [DEFAULTS.profiles, "기본값"],
    cdp: [DEFAULTS.cdp, "기본값"],
    portBase: [DEFAULTS.portBase, "기본값"],
    prefix: [DEFAULTS.prefix, "기본값"],
  };
  const envMap = { profiles: "DONGSI_E2E_PROFILES", cdp: "DONGSI_E2E_CDP", portBase: "DONGSI_E2E_PORT_BASE", prefix: "DONGSI_E2E_PREFIX" };
  for (const [k, name] of Object.entries(envMap)) {
    if (env[name] !== undefined) raw[k] = [env[name], "환경 변수 " + name];
  }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(a + " 값이 없습니다");
      return v;
    };
    if (a === "--scenario") {
      const v = next();
      args.scenario = v === "all" ? ["a", "b", "shots"] : v.split(",").map((x) => x.trim());
      for (const s of args.scenario) {
        if (!["a", "b", "shots"].includes(s)) throw new Error("알 수 없는 시나리오: " + s);
      }
    } else if (a === "--out") args.out = path.resolve(next());
    else if (a === "--shots") args.shots = path.resolve(next());
    else if (a === "--image") args.image = next();
    else if (a === "--server-addr") {
      args.serverAddr = next();
      if (!["loopback", "lan"].includes(args.serverAddr)) throw new Error("--server-addr은 loopback 또는 lan");
    } else if (a === "--keep") args.keep = true;
    else if (a === "--exe") args.exe = path.resolve(next());
    else if (a === "--profiles") raw.profiles = [next(), "인자 --profiles"];
    else if (a === "--cdp") raw.cdp = [next(), "인자 --cdp"];
    else if (a === "--port-base") raw.portBase = [next(), "인자 --port-base"];
    else if (a === "--prefix") raw.prefix = [next(), "인자 --prefix"];
    else if (a === "--help" || a === "-h") args.help = true;
    else throw new Error("알 수 없는 인자: " + a);
  }
  if (args.help) return args;
  args.cfg = resolveConfig({ profiles: raw.profiles[0], cdp: raw.cdp[0], portBase: raw.portBase[0], prefix: raw.prefix[0] });
  args.cfg.sources = Object.fromEntries(Object.entries(raw).map(([k, v]) => [k, v[1]]));
  return args;
}
