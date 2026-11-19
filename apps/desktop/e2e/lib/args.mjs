// 명령줄 인자
import path from "node:path";

export const USAGE = `사용법: node e2e/run.mjs --scenario a|b|shots|all [옵션]

  --scenario <a|b|shots|all>   돌릴 시나리오(기본 all). 쉼표로 여럿: a,shots
  --out <폴더>                 결과 폴더(기본 e2e/out/<시각>/)
  --shots <폴더>               스크린샷 폴더(기본 <out>/shots)
  --image <이름>               시나리오 b의 서버 Docker 이미지(기본 dongsi-server:e2e)
  --server-addr <loopback|lan> 시나리오 b의 서버 주소: 127.0.0.1 또는 이 PC의 LAN IPv4(기본 loopback)
  --keep                       끝난 뒤 t4- 컨테이너·볼륨을 지우지 않는다
  --exe <경로>                 앱 실행 파일(기본 <저장소>/target/debug/dongsi-desktop.exe)
  --help
`;

export function parseArgs(argv, defaults) {
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
    else if (a === "--help" || a === "-h") args.help = true;
    else throw new Error("알 수 없는 인자: " + a);
  }
  return args;
}
