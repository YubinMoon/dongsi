// 명령줄 인자(> 환경 변수 > 기본값)와 값 검사
import path from "node:path";

export const USAGE = `사용법: node e2e/run.mjs --scenario a|b|shots|av|all [옵션]

  --scenario <a|b|shots|av|all> 돌릴 시나리오(기본 all = a,b,shots). 쉼표로 여럿: a,shots. av(A/V·화면 전환 실측)는 all에 들어 있지 않고 따로만 돈다
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

A/V·화면 전환 실측(--scenario av, 인자 > 환경 변수 DONGSI_E2E_AV_* > 기본값):
  --av-targets <lan,server>    대상(기본 lan). server는 시나리오 b처럼 Docker 서버를 띄운다
  --av-codecs <h264,h264sw,av1>  코덱(기본 전부). h264sw는 A를 소프트웨어 H.264 인코더로 다시 띄운다
  --av-preset <이름>           방송 화질(기본 "원본 30fps" = 테스트 패턴 1280×720. 해상도 FHD·QHD·원본 + 주사율 30·60·144·원본)
  --av-secs <n>                코덱마다 기록하는 시간(초, 기본 25)
  --av-switches <n>            대상마다 화질·코덱 전환 횟수(7초 간격, 기본 8, 0이면 전환 시험을 건너뜀)
  --av-trace                   A를 DONGSI_MEDIA_TRACE=<out>/trace/<대상>-<코덱>-<팔>-r<반복> 으로 띄운다(송신 쪽 프레임 기록, 실행마다 A를 다시 띄움)
  --av-arms <on,off,stress>    비교 팔(기본 on). on = 립싱크 보정 켬, off = 끔, stress = 끔 + 시청 비디오 jitterBufferTarget을 주기적으로 바꿈.
                               쉼표로 여럿이면 반복마다 한 칸씩 돌려 번갈아 돈다. 인자·환경 변수가 없을 때 DONGSI_E2E_AV_LIPSYNC=off 이면 기본이 off
  --av-repeat <n>              (대상 × 코덱 × 팔) 묶음을 되풀이하는 횟수(1~20, 기본 1). 실행마다 새 방송·새 시청
  --av-stress-period <ms>      stress 팔에서 jitterBufferTarget을 바꾸는 주기(500~10000, 기본 2000)
  --av-stress-ms <ms>          stress 팔에서 올리는 jitterBufferTarget 값(20~1000, 기본 150). 0과 번갈아 바꾼다
  --help
`;

export const DEFAULTS = { profiles: "t4a,t4b", cdp: "9225,9226", portBase: "8400", prefix: "t4-" };
export const SCENARIOS = ["a", "b", "shots", "av"];
export const AV_DEFAULTS = {
  targets: "lan",
  codecs: "h264,h264sw,av1",
  preset: "원본 30fps",
  secs: "25",
  switches: "8",
  trace: "",
  arms: "on",
  repeat: "1",
  stressPeriod: "2000",
  stressMs: "150",
};
export const AV_TARGETS = ["lan", "server"];
export const AV_ARMS = ["on", "off", "stress"];
export const AV_CODECS = ["h264", "h264sw", "av1"];
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

/** --av-* 값을 검사해 돌려준다. 잘못되면 던진다(앱을 띄우기 전). */
export function resolveAv({ targets, codecs, preset, secs, switches, trace, arms = "on", repeat = "1", stressPeriod = "2000", stressMs = "150" }) {
  const list = (label, v, allowed) => {
    const xs = String(v).split(",").map((x) => x.trim()).filter(Boolean);
    if (!xs.length) throw new Error(label + " 값이 비어 있습니다");
    for (const x of xs) if (!allowed.includes(x)) throw new Error(`${label}에 알 수 없는 값: "${x}" (허용: ${allowed.join(", ")})`);
    return [...new Set(xs)];
  };
  const p = String(preset).trim();
  if (!/^[0-9A-Za-z가-힣 ]{1,32}$/.test(p)) throw new Error(`--av-preset 값이 이상합니다: "${preset}"`);
  return {
    targets: list("--av-targets", targets, AV_TARGETS),
    codecs: list("--av-codecs", codecs, AV_CODECS),
    preset: p,
    secs: intIn("--av-secs", secs, 8, 600),
    switches: intIn("--av-switches", switches, 0, 40),
    trace: trace === true || trace === "1" || trace === "true",
    arms: list("--av-arms", arms, AV_ARMS),
    repeat: intIn("--av-repeat", repeat, 1, 20),
    stressPeriod: intIn("--av-stress-period", stressPeriod, 500, 10000),
    stressMs: intIn("--av-stress-ms", stressMs, 20, 1000),
  };
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
  const avRaw = { ...AV_DEFAULTS };
  const avEnv = { targets: "DONGSI_E2E_AV_TARGETS", codecs: "DONGSI_E2E_AV_CODECS", preset: "DONGSI_E2E_AV_PRESET", secs: "DONGSI_E2E_AV_SECS", switches: "DONGSI_E2E_AV_SWITCHES", trace: "DONGSI_E2E_AV_TRACE",
    arms: "DONGSI_E2E_AV_ARMS", repeat: "DONGSI_E2E_AV_REPEAT", stressPeriod: "DONGSI_E2E_AV_STRESS_PERIOD", stressMs: "DONGSI_E2E_AV_STRESS_MS" };
  for (const [k, name] of Object.entries(avEnv)) if (env[name] !== undefined) avRaw[k] = env[name];
  // 호환: 팔을 따로 정하지 않았을 때만 DONGSI_E2E_AV_LIPSYNC=off 가 기본 팔을 off 로 바꾼다
  let armsGiven = env.DONGSI_E2E_AV_ARMS !== undefined;
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
        if (!SCENARIOS.includes(s)) throw new Error("알 수 없는 시나리오: " + s);
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
    else if (a === "--av-targets") avRaw.targets = next();
    else if (a === "--av-codecs") avRaw.codecs = next();
    else if (a === "--av-preset") avRaw.preset = next();
    else if (a === "--av-secs") avRaw.secs = next();
    else if (a === "--av-switches") avRaw.switches = next();
    else if (a === "--av-trace") avRaw.trace = true;
    else if (a === "--av-arms") {
      avRaw.arms = next();
      armsGiven = true;
    } else if (a === "--av-repeat") avRaw.repeat = next();
    else if (a === "--av-stress-period") avRaw.stressPeriod = next();
    else if (a === "--av-stress-ms") avRaw.stressMs = next();
    else if (a === "--help" || a === "-h") args.help = true;
    else throw new Error("알 수 없는 인자: " + a);
  }
  if (args.help) return args;
  if (!armsGiven && env.DONGSI_E2E_AV_LIPSYNC === "off") avRaw.arms = "off";
  args.av = resolveAv(avRaw);
  args.cfg = resolveConfig({ profiles: raw.profiles[0], cdp: raw.cdp[0], portBase: raw.portBase[0], prefix: raw.prefix[0] });
  args.cfg.sources = Object.fromEntries(Object.entries(raw).map(([k, v]) => [k, v[1]]));
  return args;
}
