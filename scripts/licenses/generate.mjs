#!/usr/bin/env node
// 제3자 고지(THIRD_PARTY_NOTICES.md) 생성기. Windows에서 돈다(PowerShell 또는 bash에서도 같다).
//
//   node scripts/licenses/generate.mjs [--ffmpeg <FFmpeg 폴더>] [--out <파일>] [--check]
//
// - --ffmpeg   FFmpeg LGPL 빌드 폴더(scripts/ffmpeg/build.sh의 결과). 기본은 환경 변수 FFMPEG_DIR.
//              <폴더>/licenses/components.json이 없거나 configure에 --enable-gpl·--enable-nonfree가 있으면 오류로 끝난다.
// - --out      결과 파일. 기본은 저장소 루트의 THIRD_PARTY_NOTICES.md.
// - --check    파일을 쓰지 않고, 지금 만들어지는 내용이 --out 파일과 같은지만 본다(다르면 종료 코드 1).
// - --npm-report <파일>   (디버깅) 프런트엔드를 다시 빌드하지 않고 이 패키지 목록을 쓴다.
// - --cargo-json <파일>   (디버깅) cargo-about를 다시 돌리지 않고 이 JSON을 쓴다.
//
// 구성(자세한 설명은 docs/licensing.md):
//   1. FFmpeg와 그 안에 넣은 라이브러리: <FFmpeg 폴더>/licenses/components.json과 라이선스 파일
//   2. Pretendard 글꼴: apps/desktop/static/licenses/Pretendard-OFL-1.1.txt
//   3. npm 패키지: apps/desktop을 vite로 빌드하면서 번들에 실제로 들어간 패키지(apps/desktop/vite-bundle-report.js)
//   4. Rust 크레이트: cargo-about(scripts/licenses/about.toml)가 센 것. 우리 워크스페이스 크레이트는 뺀다.
// 결과는 실행할 때마다 같다(정렬, 날짜·절대 경로 없음). 네트워크를 쓰지 않는다(cargo-about --offline).

import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..", "..");
const desktop = path.join(root, "apps", "desktop");

const RELEASES_URL = "https://github.com/YubinMoon/dongsi/releases";
const REPO_URL = "https://github.com/YubinMoon/dongsi";

// ---------- 인자 ----------

function parseArgs(argv) {
  const a = { ffmpeg: process.env.FFMPEG_DIR ?? "", out: path.join(root, "THIRD_PARTY_NOTICES.md"), check: false };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => {
      if (i + 1 >= argv.length) fail("인자 " + k + "에 값이 없습니다.");
      return argv[++i];
    };
    if (k === "--ffmpeg") a.ffmpeg = v();
    else if (k === "--out") a.out = path.resolve(v());
    else if (k === "--check") a.check = true;
    else if (k === "--npm-report") a.npmReport = path.resolve(v());
    else if (k === "--cargo-json") a.cargoJson = path.resolve(v());
    else if (k === "-h" || k === "--help") {
      const head = fs.readFileSync(fileURLToPath(import.meta.url), "utf8").split("\n").slice(1, 19);
      console.log(head.map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
      process.exit(0);
    } else fail("알 수 없는 인자: " + k);
  }
  return a;
}

function fail(msg) {
  console.error("오류: " + msg);
  process.exit(1);
}

// ---------- 도우미 ----------

/** 코드 단위 비교(로케일에 기대지 않아 어디서 돌려도 같은 순서). */
const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

/** 줄 끝을 LF로, 앞뒤 빈 줄·줄 끝 공백을 지운다. */
function tidy(text) {
  if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
  return text
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((l) => l.replace(/[ \t]+$/, ""))
    .join("\n")
    .replace(/^\n+/, "")
    .replace(/\n+$/, "");
}

/** 공백 차이만 있는 같은 문구를 한 묶음으로 보기 위한 열쇠. */
const textKey = (text) => text.replace(/\s+/g, " ").trim();

const sha256 = (s) => crypto.createHash("sha256").update(s).digest("hex");

/** 본문에 나오는 가장 긴 백틱 줄보다 한 칸 긴 울타리로 감싼다. */
function fence(text) {
  let n = 3;
  for (const m of text.matchAll(/`+/g)) n = Math.max(n, m[0].length + 1);
  const f = "`".repeat(n);
  return f + "text\n" + text + "\n" + f;
}

const code = (s) => "`" + s + "`";

function run(cmd, args, opts = {}) {
  return spawnSync(cmd, args, { encoding: "utf8", maxBuffer: 256 * 1024 * 1024, ...opts });
}

// ---------- 1. FFmpeg ----------

function loadFfmpeg(dir) {
  if (!dir) fail("FFmpeg 폴더를 알 수 없습니다. --ffmpeg <폴더>를 주거나 환경 변수 FFMPEG_DIR을 설정하세요.");
  const licDir = path.join(dir, "licenses");
  const manifest = path.join(licDir, "components.json");
  if (!fs.existsSync(manifest)) {
    fail(
      "FFmpeg 폴더에 licenses/components.json이 없습니다: " + manifest +
        "\n  LGPL 빌드가 아니거나(예: BtbN GPL 빌드) scripts/ffmpeg/build.sh로 만든 폴더가 아닙니다. 고지를 만들지 않습니다.",
    );
  }
  const data = JSON.parse(fs.readFileSync(manifest, "utf8"));
  const configure = data.ffmpeg?.configure ?? "";
  if (!configure) fail("components.json에 ffmpeg.configure가 없습니다.");
  const opts = configure.split(/\s+/);
  for (const bad of ["--enable-gpl", "--enable-nonfree", "--enable-libx264", "--enable-libx265", "--enable-libxvid"]) {
    if (opts.includes(bad)) fail("FFmpeg configure에 " + bad + "가 있습니다. GPL·nonfree 빌드는 배포할 수 없습니다(D-062).");
  }
  if (!Array.isArray(data.components) || data.components.length === 0) fail("components.json에 components가 없습니다.");
  if (!data.sourceBundle?.file || !/^[0-9a-f]{64}$/.test(data.sourceBundle.sha256 ?? "")) {
    fail("components.json의 sourceBundle(file, sha256)이 올바르지 않습니다.");
  }
  const dlls = fs.existsSync(path.join(dir, "bin"))
    ? fs.readdirSync(path.join(dir, "bin")).filter((f) => f.toLowerCase().endsWith(".dll")).sort(cmp)
    : [];
  if (dlls.length === 0) fail("FFmpeg 폴더의 bin/에 DLL이 없습니다: " + path.join(dir, "bin"));

  // 라이선스 파일을 읽어 같은 문구는 한 번만 싣는다.
  const texts = new Map(); // key -> { text, files, components }
  for (const c of data.components) {
    for (const rel of c.licenseFiles ?? []) {
      const file = path.join(licDir, rel);
      if (!fs.existsSync(file)) fail("라이선스 파일이 없습니다: licenses/" + rel + " (구성 요소 " + c.id + ")");
      const text = tidy(fs.readFileSync(file, "utf8"));
      const key = sha256(textKey(text));
      if (!texts.has(key)) texts.set(key, { text, files: new Set(), components: new Set() });
      const t = texts.get(key);
      t.files.add(rel.replace(/\\/g, "/"));
      t.components.add(c.name ?? c.id);
    }
  }
  const hasLgpl21 = [...texts.values()].some((t) => /GNU LESSER GENERAL PUBLIC LICENSE\s+Version 2\.1/i.test(t.text));
  if (!hasLgpl21) fail("licenses/ 안에 LGPL 2.1 전문(COPYING.LGPLv2.1)이 없습니다.");
  return { data, dlls, texts: [...texts.values()].sort((a, b) => cmp([...a.files][0], [...b.files][0])) };
}

function renderFfmpeg(f) {
  const { data, dlls, texts } = f;
  const out = [];
  const ff = data.components.find((c) => c.id === "ffmpeg") ?? data.components[0];
  const cell = (s) => String(s ?? "").replace(/\|/g, "\\|").replace(/\s*\n\s*/g, " ");
  out.push("## 1. FFmpeg와 그 안에 넣은 라이브러리", "");
  out.push(
    "**이 프로그램은 LGPL 2.1 이상으로 배포되는 FFmpeg를 사용합니다.** " +
      "FFmpeg(" + data.ffmpeg.version + ")는 GNU Lesser General Public License 버전 2.1 또는 그 이후 버전(LGPL-2.1-or-later)으로 배포됩니다. " +
      "dongsi가 쓰는 FFmpeg는 GPL 부품(libx264·libx265 등)과 nonfree 부품을 넣지 않고 만든 LGPL 빌드입니다.",
    "",
  );
  out.push(
    "- 연결 방식: 공유 라이브러리(DLL)입니다. 앱 실행 파일(" + code("dongsi.exe") + ")에 합쳐 넣지 않고, 실행 파일 옆의 DLL을 불러 씁니다: " +
      dlls.map(code).join(", ") + ".",
  );
  out.push("- FFmpeg 홈페이지: " + (ff.homepage ?? "https://ffmpeg.org/"));
  out.push("", "### 들어 있는 구성 요소", "");
  out.push("| 이름 | 버전 | 라이선스 | 연결 방식 | 소스 |", "|---|---|---|---|---|");
  for (const c of data.components) {
    out.push("| " + [c.name ?? c.id, c.version, c.license, c.linkage, c.source].map(cell).join(" | ") + " |");
  }
  const noted = data.components.filter((c) => c.notes);
  if (noted.length) {
    out.push("", "비고:", "");
    for (const c of noted) out.push("- " + (c.name ?? c.id) + ": " + String(c.notes).replace(/\s*\n\s*/g, " "));
  }
  out.push("", "### FFmpeg 소스 위치", "");
  out.push(
    "LGPL에 따라, 배포한 DLL을 만든 소스(FFmpeg와 위 구성 요소의 원본 소스, 빌드에 쓴 패치와 스크립트)를 하나로 묶어 공개합니다.",
    "",
    "- 소스 묶음 파일: " + code(data.sourceBundle.file),
    "- SHA-256: " + code(data.sourceBundle.sha256),
    "- 내려받는 곳: " + RELEASES_URL + " 의 릴리스 첨부 파일(포터블 zip을 받은 릴리스에 같이 있습니다).",
    "- 다시 만드는 방법: 저장소(" + REPO_URL + ")의 " + code("scripts/ffmpeg/") + " 폴더(진입점 " + code("build.sh") +
      ", WSL의 Docker에서 실행)가 같은 DLL을 소스 묶음에서 다시 만듭니다. 절차는 저장소의 " + code("docs/licensing.md") + "에 있습니다.",
    "",
    "원본 소스 위치와 해시:",
    "",
    "| 구성 요소 | 원본 소스 | SHA-256 |",
    "|---|---|---|",
  );
  for (const c of data.components) {
    out.push("| " + [c.name ?? c.id, c.source, c.sha256 ? code(c.sha256) : ""].map(cell).join(" | ") + " |");
  }
  out.push("", "### DLL을 바꿀 수 있습니다", "");
  out.push(
    "LGPL 2.1에 따라 사용자는 이 프로그램에 연결된 FFmpeg 라이브러리를 수정한 버전이나 다른 호환 빌드로 바꿀 수 있습니다. " +
      "위 소스 묶음으로 직접 만든 DLL(같은 이름, 같은 FFmpeg 주 버전의 ABI)을 " + code("dongsi.exe") +
      " 옆의 같은 이름 DLL 자리에 덮어쓰면 앱이 그것을 씁니다. 앱을 다시 빌드할 필요가 없습니다. " +
      "이름이나 ABI가 다른 DLL로 바꾸면 앱이 시작하지 못할 수 있습니다.",
  );
  out.push("", "### FFmpeg configure 줄", "", "다음 configure 줄로 빌드했습니다.", "", fence(data.ffmpeg.configure));
  out.push("", "### 라이선스 전문(FFmpeg와 구성 요소)", "");
  for (const t of texts) {
    out.push("#### " + [...t.files].join(", "), "", "사용하는 구성 요소: " + [...t.components].join(", "), "", fence(t.text), "");
  }
  return out;
}

// ---------- 2. Pretendard ----------

function renderPretendard() {
  const file = path.join(desktop, "static", "licenses", "Pretendard-OFL-1.1.txt");
  const text = tidy(fs.readFileSync(file, "utf8"));
  const lock = JSON.parse(fs.readFileSync(path.join(desktop, "package-lock.json"), "utf8"));
  const ver = lock.packages?.["node_modules/pretendard"]?.version ?? "?";
  return [
    "## 2. Pretendard (글꼴)",
    "",
    "앱 화면의 글꼴로 Pretendard Variable(npm 패키지 " + code("pretendard") + " " + ver + ")을 앱에 함께 넣어 배포합니다. " +
      "SIL Open Font License 1.1(OFL-1.1)로 배포되는 글꼴이며, 글꼴 파일은 수정하지 않았습니다. 프로젝트: https://github.com/orioncactus/pretendard",
    "",
    fence(text),
  ];
}

// ---------- 3. npm ----------

function findLicenseFiles(dir) {
  return fs
    .readdirSync(dir)
    .filter((f) => /^(licen[sc]e|copying|notice|unlicense)([-._].*)?$/i.test(f) && fs.statSync(path.join(dir, f)).isFile())
    .sort(cmp);
}

function licenseOfPackageJson(pkg) {
  const l = pkg.license ?? pkg.licenses;
  if (typeof l === "string") return l;
  if (Array.isArray(l)) return l.map((x) => (typeof x === "string" ? x : x.type)).join(" OR ");
  if (l && typeof l === "object") return l.type ?? "?";
  return "?";
}

function collectNpm(args) {
  let reportFile = args.npmReport;
  let tmp = null;
  if (!reportFile) {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "dongsi-notices-"));
    reportFile = path.join(tmp, "bundle-report.json");
    console.error("[npm] 프런트엔드를 빌드하면서 번들에 든 패키지를 모읍니다…");
    if (!fs.existsSync(path.join(desktop, "node_modules"))) fail("apps/desktop/node_modules가 없습니다. apps/desktop에서 npm ci를 먼저 실행하세요.");
    const r = spawnSync("npm run build", {
      cwd: desktop,
      shell: true,
      encoding: "utf8",
      env: { ...process.env, DONGSI_BUNDLE_REPORT: reportFile },
      maxBuffer: 64 * 1024 * 1024,
    });
    if (r.status !== 0) fail("프런트엔드 빌드에 실패했습니다.\n" + (r.stdout ?? "") + (r.stderr ?? ""));
  }
  if (!fs.existsSync(reportFile)) fail("번들 보고 파일이 만들어지지 않았습니다: " + reportFile);
  const report = JSON.parse(fs.readFileSync(reportFile, "utf8"));
  if (tmp) fs.rmSync(tmp, { recursive: true, force: true });
  const pkgs = [];
  for (const p of report) {
    if (p.name === "pretendard") continue; // 2절에서 따로 싣는다
    const manifest = JSON.parse(fs.readFileSync(path.join(p.dir, "package.json"), "utf8"));
    const files = findLicenseFiles(p.dir);
    if (files.length === 0) fail("npm 패키지 " + p.name + "@" + p.version + "에 라이선스 파일이 없습니다.");
    pkgs.push({
      name: p.name,
      version: p.version,
      license: licenseOfPackageJson(manifest),
      texts: files.map((f) => tidy(fs.readFileSync(path.join(p.dir, f), "utf8"))),
    });
  }
  pkgs.sort((a, b) => cmp(a.name, b.name) || cmp(a.version, b.version));
  return pkgs;
}

function renderNpm(pkgs) {
  const out = ["## 3. npm 패키지 (화면 코드에 들어가는 것)", ""];
  out.push(
    "앱 화면(SvelteKit)의 빌드 결과(" + code("apps/desktop/build/") + ")에 실제로 들어간 패키지만 적습니다. " +
      "빌드 도구(vite, typescript 등)는 배포물에 들어가지 않아 뺍니다.",
    "",
    "| 패키지 | 버전 | 라이선스 |",
    "|---|---|---|",
  );
  for (const p of pkgs) out.push("| " + p.name + " | " + p.version + " | " + p.license + " |");
  out.push("");
  const groups = new Map();
  for (const p of pkgs) {
    const joined = p.texts.join("\n\n----\n\n");
    const key = sha256(textKey(joined));
    if (!groups.has(key)) groups.set(key, { text: joined, pkgs: [] });
    groups.get(key).pkgs.push(p.name + " " + p.version);
  }
  const sorted = [...groups.values()].sort((a, b) => cmp(a.pkgs[0], b.pkgs[0]));
  for (const g of sorted) out.push("### " + g.pkgs.join(", "), "", fence(g.text), "");
  return out;
}

// ---------- 4. Rust ----------

function collectRust(args) {
  let jsonFile = args.cargoJson;
  let tmp = null;
  if (!jsonFile) {
    const v = run("cargo", ["about", "--version"]);
    if (v.error || v.status !== 0) fail("cargo-about가 없습니다. 먼저 설치하세요:\n  cargo install cargo-about --locked");
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "dongsi-notices-"));
    jsonFile = path.join(tmp, "about.json");
    console.error("[rust] " + v.stdout.trim() + "로 크레이트 라이선스를 모읍니다…");
    const r = run(
      "cargo",
      ["about", "generate", "--workspace", "--format", "json", "--locked", "--offline", "-c", path.join(here, "about.toml"), "-o", jsonFile],
      { cwd: root },
    );
    if (r.status !== 0) {
      fail(
        "cargo-about가 실패했습니다. 허용 목록(scripts/licenses/about.toml의 accepted)에 없는 라이선스일 수 있습니다." +
          "\n  새 라이선스를 허용하기 전에 copyleft(GPL·LGPL 등)인지 확인하고 docs/licensing.md에 적으세요.\n" +
          (r.stdout ?? "") + (r.stderr ?? ""),
      );
    }
  }
  const raw = JSON.parse(fs.readFileSync(jsonFile, "utf8"));
  if (tmp) fs.rmSync(tmp, { recursive: true, force: true });

  // 우리 워크스페이스 크레이트는 source가 없다.
  const crates = raw.crates
    .filter((c) => c.package.source)
    .map((c) => ({ name: c.package.name, version: c.package.version, license: c.license }))
    .sort((a, b) => cmp(a.name, b.name) || cmp(a.version, b.version));

  const groups = new Map();
  for (const l of raw.licenses) {
    const used = l.used_by
      .filter((u) => u.crate.source)
      .map((u) => ({ name: u.crate.name, version: u.crate.version, authors: u.crate.authors ?? [] }));
    if (used.length === 0) continue;
    const text = tidy(l.text);
    const placeholder = /<year>|<owner>|<copyright holders>/i.test(text);
    const key = l.id + "\x00" + textKey(text);
    if (!groups.has(key)) groups.set(key, { id: l.id, name: l.name, text, placeholder, used: new Map() });
    const g = groups.get(key);
    for (const u of used) g.used.set(u.name + "@" + u.version, u);
  }
  const lic = [...groups.values()]
    .map((g) => ({ ...g, used: [...g.used.values()].sort((a, b) => cmp(a.name, b.name) || cmp(a.version, b.version)) }))
    .sort((a, b) => cmp(a.id, b.id) || cmp(a.used[0].name, b.used[0].name) || cmp(textKey(a.text), textKey(b.text)));
  return { crates, lic };
}

function renderRust(r) {
  const out = ["## 4. Rust 크레이트", ""];
  out.push(
    "데스크톱 앱(" + code("dongsi.exe") + ", Windows x64)과 전용 서버(" + code("dongsi-server") + ", Windows x64·Linux x64)에 들어가는 크레이트입니다. " +
      "빌드 스크립트·시험에만 쓰는 크레이트와 dongsi 자체 크레이트는 뺐습니다. " +
      "소스는 https://crates.io/crates/<이름>/<버전> 에서 받을 수 있습니다(MPL-2.0 크레이트도 같습니다. dongsi는 이 크레이트들을 수정하지 않았습니다).",
    "",
    "크레이트 " + r.crates.length + "개.",
    "",
    "| 크레이트 | 버전 | 라이선스 |",
    "|---|---|---|",
  );
  for (const c of r.crates) out.push("| " + c.name + " | " + c.version + " | " + c.license + " |");
  out.push("", "### 라이선스 전문", "");
  out.push("같은 문구는 한 번만 적고, 그 문구를 쓰는 크레이트를 함께 나열합니다.", "");
  for (const g of r.lic) {
    out.push("#### " + g.name + " (" + g.id + ")", "");
    if (g.placeholder) {
      out.push(
        "이 크레이트들의 패키지에는 저작권자가 적힌 라이선스 파일이 없습니다. 아래는 표준 문구(저작권자 자리가 비어 있음)이고, 크레이트별 저작권자는 각 크레이트 Cargo.toml의 " +
          code("authors") + " 항목입니다.",
        "",
      );
      for (const u of g.used) out.push("- " + u.name + " " + u.version + (u.authors.length ? " — " + u.authors.join("; ") : ""));
    } else {
      out.push("사용하는 크레이트: " + g.used.map((u) => u.name + " " + u.version).join(", "));
    }
    out.push("", fence(g.text), "");
  }
  return out;
}

// ---------- 머리말 ----------

function header() {
  return [
    "# 제3자 고지 (THIRD_PARTY_NOTICES)",
    "",
    "dongsi는 MIT 라이선스(저장소 루트의 " + code("LICENSE") + ")로 공개하는 프로그램이고, 아래 제3자 소프트웨어를 사용합니다. 이 문서는 그 라이선스 고지입니다.",
    "",
    "- 1. FFmpeg와 그 안에 넣은 라이브러리 (LGPL 2.1 이상 + 구성 요소별 라이선스)",
    "- 2. Pretendard (글꼴, OFL-1.1)",
    "- 3. npm 패키지 (화면 코드에 들어가는 것)",
    "- 4. Rust 크레이트",
    "",
    "이 파일은 생성물입니다. 직접 고치지 말고 " + code("node scripts/licenses/generate.mjs --ffmpeg <FFmpeg 폴더>") +
      "로 다시 만드세요(FFmpeg 폴더는 환경 변수 " + code("FFMPEG_DIR") + "로도 줄 수 있습니다). 필요한 것: Node.js, " + code("apps/desktop") +
      "의 " + code("npm ci") + ", cargo-about(" + code("cargo install cargo-about --locked") + "). 의존성이나 FFmpeg 빌드가 바뀌면 다시 만듭니다.",
    "",
  ];
}

// ---------- 본체 ----------

const args = parseArgs(process.argv.slice(2));
const ffmpeg = loadFfmpeg(args.ffmpeg);
const npm = collectNpm(args);
const rust = collectRust(args);

const lines = [
  ...header(),
  ...renderFfmpeg(ffmpeg),
  "",
  ...renderPretendard(),
  "",
  ...renderNpm(npm),
  ...renderRust(rust),
];
const content = lines.join("\n").replace(/\n{3,}/g, "\n\n").replace(/\n+$/, "") + "\n";

if (args.check) {
  const cur = fs.existsSync(args.out) ? fs.readFileSync(args.out, "utf8").replace(/\r\n/g, "\n") : "";
  if (cur !== content) fail(path.relative(root, args.out) + "이(가) 최신이 아닙니다. 다시 만드세요: node scripts/licenses/generate.mjs");
  console.error("최신입니다: " + path.relative(root, args.out));
} else {
  fs.writeFileSync(args.out, content);
  console.error(
    "썼습니다: " + args.out + " (" + content.length + "자) — 크레이트 " + rust.crates.length + "개, npm " + npm.length + "개(" +
      npm.map((p) => p.name).join(", ") + "), FFmpeg 구성 요소 " + ffmpeg.data.components.length + "개",
  );
}
