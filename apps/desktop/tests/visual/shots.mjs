// 화면 비교 하네스 실행기: Rust 앱 없이 build/를 브라우저(설치된 Edge)에서 띄워 시나리오마다 화면을 찍는다.
//
//   npm run build
//   npm run shots -- --out <출력 폴더> [--design <비교할 PNG 폴더>] [--only b-home,b-live] [--url http://127.0.0.1:8150] [--headed]
//
// 자세한 설명은 tests/visual/README.md.
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium } from "playwright-core";
import { buildFixtures } from "./fixtures.mjs";
import { createScenarios } from "./scenarios.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DESKTOP = path.resolve(HERE, "../..");
const BUILD_DIR = path.join(DESKTOP, "build");
const PORT_RANGE = [8150, 8159];
const ACTION_TIMEOUT = 5000;

// ---------- 인자 ----------
function parseArgs(argv) {
  const args = { out: path.join(HERE, "out"), design: null, only: null, url: null, headed: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(a + " 값이 없습니다");
      return v;
    };
    if (a === "--out") args.out = path.resolve(next());
    else if (a === "--design") args.design = path.resolve(next());
    else if (a === "--only") args.only = next().split(",").map((x) => x.trim()).filter(Boolean);
    else if (a === "--url") args.url = next().replace(/#.*$/, "");
    else if (a === "--headed") args.headed = true;
    else if (a === "--help" || a === "-h") {
      console.log("사용법: npm run shots -- [--out 폴더] [--design 비교할 PNG 폴더] [--only 이름,이름] [--url 주소] [--headed]");
      process.exit(0);
    } else throw new Error("알 수 없는 인자: " + a);
  }
  return args;
}

// ---------- build/를 서빙하는 작은 정적 서버(포트 8150~8159) ----------
const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".txt": "text/plain; charset=utf-8",
};

function startStaticServer(root) {
  const server = http.createServer((req, res) => {
    const urlPath = decodeURIComponent((req.url ?? "/").split("?")[0]);
    let file = path.join(root, path.normalize(urlPath));
    if (!file.startsWith(root)) {
      res.writeHead(403).end();
      return;
    }
    if (!fs.existsSync(file) || fs.statSync(file).isDirectory()) {
      const index = path.join(file, "index.html");
      file = fs.existsSync(index) && fs.statSync(file).isDirectory() ? index : path.join(root, "index.html");
    }
    res.writeHead(200, {
      "content-type": MIME[path.extname(file).toLowerCase()] ?? "application/octet-stream",
      "cache-control": "no-store",
    });
    fs.createReadStream(file).pipe(res);
  });
  return new Promise((resolve, reject) => {
    let port = PORT_RANGE[0];
    const tryListen = () => {
      server.once("error", (e) => {
        if (e.code === "EADDRINUSE" && port < PORT_RANGE[1]) {
          port++;
          tryListen();
        } else reject(e);
      });
      server.listen(port, "127.0.0.1", () => resolve({ server, url: "http://127.0.0.1:" + port }));
    };
    tryListen();
  });
}

// ---------- 대상 → Playwright locator ----------
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function locate(page, t) {
  if (t.or) return t.or.map((x) => locate(page, x)).reduce((a, b) => a.or(b)).first();
  if (t.role) return page.getByRole(t.role, { name: t.name, exact: t.exact }).first();
  if (t.label) return page.getByLabel(t.label, { exact: t.exact }).first();
  if (t.text) return page.getByText(t.text, { exact: t.exact }).first();
  if (t.css) return page.locator(t.css).first();
  if (t.card) return page.locator('[data-server-id="' + t.card + '"]').first();
  if (t.cardButton) {
    const card = page.locator('[data-server-id="' + t.cardButton + '"]').first();
    // ⋯ 메뉴 단추("'이름' 서버 메뉴")에도 서버 이름이 들어 있으니 뺀다.
    const re = new RegExp("^(?!.*서버 메뉴).*" + escapeRe(t.name));
    return card.getByRole("button", { name: re }).or(card.getByRole("link", { name: re })).first();
  }
  if (t.cardMenu) return page.getByRole("button", { name: "'" + t.cardMenu + "' 서버 메뉴", exact: true }).first();
  if (t.menuitem) return page.getByRole("menuitem", { name: t.menuitem }).first();
  if (t.tile) return page.getByRole("link", { name: t.tile }).first();
  throw new Error("알 수 없는 대상: " + JSON.stringify(t));
}

const describeTarget = (t) => JSON.stringify(t);

async function runStep(page, step) {
  if (step.waitFor) return locate(page, step.waitFor).waitFor({ state: "visible" });
  if (step.click) return locate(page, step.click).click();
  if (step.fill) return locate(page, step.fill).fill(step.value);
  if (step.hover) return locate(page, step.hover).hover();
  if (step.press) return page.keyboard.press(step.press);
  if (step.wait !== undefined) return page.waitForTimeout(step.wait);
  if (step.emit) return page.evaluate(([e, p]) => window.__mockEmit(e, p), step.emit);
  if (step.hash) return page.evaluate((h) => (location.hash = h), step.hash);
  if (step.eval) return page.evaluate(step.eval);
  throw new Error("알 수 없는 동작: " + JSON.stringify(step));
}

// ---------- 시나리오 하나 ----------
async function runScenario(browser, baseUrl, sc, fx, outDir) {
  const started = Date.now();
  const result = {
    name: sc.name,
    note: sc.note ?? "",
    status: "ok",
    error: null,
    failedStep: null,
    unhandled: [],
    consoleErrors: [],
    size: sc.viewport,
    contentHeight: null,
    fontOk: null,
    ms: 0,
  };
  const context = await browser.newContext({
    viewport: sc.viewport,
    deviceScaleFactor: 1,
    locale: "ko-KR",
    colorScheme: "dark",
  });
  const page = await context.newPage();
  page.setDefaultTimeout(ACTION_TIMEOUT);
  page.on("console", (msg) => {
    if (msg.type() !== "error") return;
    if (/favicon/.test(msg.location().url ?? "")) return;
    result.consoleErrors.push(msg.text());
  });
  page.on("pageerror", (e) => result.consoleErrors.push("(처리 안 된 예외) " + e.message));

  try {
    // 방송 시작 시각이 "N분 전"으로 고정되게 Date만 멈춘다(타이머는 그대로 돈다).
    await page.clock.setFixedTime(new Date(fx.now * 1000));
    const config = {
      table: sc.table,
      state: sc.state,
      windowLabel: "main",
    };
    await page.addInitScript({ content: "window.__MOCK_CONFIG__ = " + JSON.stringify(config) + ";" });
    await page.addInitScript({ path: path.join(HERE, "mock-ipc.js") });
    await page.goto(baseUrl + "/" + sc.hash, { waitUntil: "load" });

    for (const step of sc.steps) {
      try {
        await runStep(page, step);
      } catch (e) {
        result.failedStep = step;
        throw new Error(
          "동작 실패 " + describeTarget(step) + ": " + String(e.message).split("\n")[0],
        );
      }
    }
  } catch (e) {
    result.status = "fail";
    result.error = String(e.message ?? e);
  }

  // 실패해도 그 시점의 화면을 찍어 비교 화면에서 볼 수 있게 한다.
  try {
    await page.evaluate(() => document.fonts.ready);
    result.fontOk = await page.evaluate(() => document.fonts.check('14px "Pretendard Variable"'));
    await page.evaluate(
      () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))),
    );
    await page.waitForTimeout(250);
    result.unhandled = [...new Set((await page.evaluate(() => window.__mock?.unhandled)) ?? [])];
    result.contentHeight = await page.evaluate(() => document.documentElement.scrollHeight);
    await page.screenshot({
      path: path.join(outDir, sc.name + ".png"),
      animations: "disabled",
      caret: "hide",
    });
  } catch (e) {
    result.status = "fail";
    result.error ??= "찍지 못했습니다: " + String(e.message).split("\n")[0];
  }

  if (result.unhandled.length > 0 && result.status === "ok") {
    result.status = "fail";
    result.error = "시나리오에 없는 명령: " + result.unhandled.join(", ");
  }
  await context.close();
  result.ms = Date.now() - started;
  return result;
}

// ---------- 비교 HTML ----------
function pngSize(file) {
  try {
    const buf = fs.readFileSync(file);
    if (buf.toString("ascii", 1, 4) !== "PNG") return null;
    return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
  } catch {
    return null;
  }
}

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/** compare.html에서 이미지로 가는 주소: 같은 드라이브면 상대 경로, 아니면 file:// */
function imgSrc(outDir, file) {
  const rel = path.relative(outDir, file);
  if (path.isAbsolute(rel)) return pathToFileURL(file).href;
  return rel.split(path.sep).map(encodeURIComponent).join("/");
}

function writeCompare(outDir, designDir, results) {
  const rows = results
    .map((r) => {
      const app = path.join(outDir, r.name + ".png");
      const design = designDir ? path.join(designDir, r.name + ".png") : null;
      const appSize = pngSize(app);
      const designSize = design && fs.existsSync(design) ? pngSize(design) : null;
      const dim = (s) => (s ? s.width + "×" + s.height : "없음");
      const sizeNote =
        appSize && designSize && (appSize.width !== designSize.width || appSize.height !== designSize.height)
          ? ' <span class="warn">크기가 다름</span>'
          : "";
      const issues = [
        r.error ? "<li class='bad'>" + esc(r.error) + "</li>" : "",
        ...r.unhandled.map((u) => "<li class='bad'>처리 안 된 명령: " + esc(u) + "</li>"),
        ...r.consoleErrors.map((c) => "<li class='warn'>콘솔: " + esc(c.slice(0, 300)) + "</li>"),
        r.contentHeight && r.contentHeight > r.size.height
          ? "<li class='warn'>내용이 창보다 김(문서 높이 " + r.contentHeight + "px, 창 " + r.size.height + "px)</li>"
          : "",
        r.fontOk === false ? "<li class='warn'>Pretendard Variable 글꼴이 로드되지 않음</li>" : "",
      ].join("");
      return `<section id="${esc(r.name)}">
  <h2>${esc(r.name)} <span class="${r.status === "ok" ? "ok" : "bad"}">${r.status === "ok" ? "성공" : "실패"}</span></h2>
  <p class="note">${esc(r.note)}</p>
  ${issues ? "<ul>" + issues + "</ul>" : ""}
  <div class="pair" data-mode="side">
    <figure><figcaption>기준 <small>${dim(designSize)}</small></figcaption>
      ${designSize ? '<img class="design" src="' + esc(imgSrc(outDir, design)) + '" alt="기준 ' + esc(r.name) + '">' : '<div class="missing">기준 PNG 없음</div>'}</figure>
    <figure><figcaption>앱 <small>${dim(appSize)}</small>${sizeNote}</figcaption>
      ${appSize ? '<img class="app" src="' + esc(imgSrc(outDir, app)) + '" alt="앱 ' + esc(r.name) + '">' : '<div class="missing">찍지 못함</div>'}</figure>
  </div>
  <p class="tools"><button type="button" data-toggle>겹쳐 보기(차이)</button></p>
</section>`;
    })
    .join("\n");
  const html = `<!doctype html>
<html lang="ko"><head><meta charset="utf-8"><title>dongsi 화면 비교</title>
<style>
  body{background:#0d0e11;color:#f3f4f6;font:14px system-ui,'Malgun Gothic',sans-serif;margin:0;padding:24px}
  h1{font-size:20px} h2{font-size:16px;margin:32px 0 4px} .note{color:#a9abb3;margin:0 0 8px}
  .ok{color:#7fdcaf} .bad{color:#ff8a8a} .warn{color:#f5d58a}
  ul{margin:0 0 8px;padding-left:20px;font-size:13px}
  .pair{display:flex;gap:16px;align-items:flex-start}
  .pair figure{margin:0;flex:1 1 0;min-width:0}
  .pair figcaption{margin-bottom:6px;font-weight:600} small{color:#8b8e97;font-weight:400}
  .pair img{width:100%;height:auto;display:block;outline:1px solid rgba(255,255,255,.12)}
  .missing{padding:40px;border:1px dashed rgba(255,255,255,.2);color:#8b8e97;text-align:center}
  .pair[data-mode=diff]{display:block;position:relative;max-width:1100px}
  .pair[data-mode=diff] figure{position:absolute;inset:0;margin:0}
  .pair[data-mode=diff] figure:first-child{position:relative}
  .pair[data-mode=diff] figure:last-child img{mix-blend-mode:difference}
  .pair[data-mode=diff] figcaption{display:none}
  button{background:#26272d;color:#f3f4f6;border:0;border-radius:8px;padding:6px 12px;cursor:pointer}
</style></head><body>
<h1>dongsi 화면 비교 <small>${new Date().toLocaleString("ko-KR")}</small></h1>
<p class="note">${results.filter((r) => r.status === "ok").length}/${results.length} 성공 · 실제 재생 화면(b-watch, b-watch-popout)은 실제 WebRTC가 필요해 실제 앱(CDP 하네스)에서 찍는다. 여기의 b-watch-playing*은 가짜 미디어다.</p>
${rows}
<script>
document.querySelectorAll('[data-toggle]').forEach((b) => b.addEventListener('click', () => {
  const pair = b.closest('section').querySelector('.pair');
  pair.dataset.mode = pair.dataset.mode === 'diff' ? 'side' : 'diff';
}));
</script></body></html>`;
  fs.writeFileSync(path.join(outDir, "compare.html"), html);
}

// ---------- 결과 표 ----------
function printTable(results) {
  const line = "─".repeat(78);
  console.log("\n" + line);
  for (const r of results) {
    const mark = r.status === "ok" ? "성공" : "실패";
    console.log(mark + "  " + r.name.padEnd(24) + String(r.ms).padStart(6) + " ms");
    if (r.error) console.log("      ↳ " + r.error);
    for (const u of r.unhandled) console.log("      ↳ 처리 안 된 명령: " + u);
    for (const c of r.consoleErrors) console.log("      ↳ 콘솔 오류: " + c.replace(/\s+/g, " ").slice(0, 200));
    if (r.contentHeight && r.contentHeight > r.size.height)
      console.log("      ↳ (참고) 내용이 창보다 김: 문서 " + r.contentHeight + "px > 창 " + r.size.height + "px");
  }
  console.log(line);
  const failed = results.filter((r) => r.status !== "ok").length;
  console.log("합계: " + (results.length - failed) + "/" + results.length + " 성공, " + failed + " 실패");
}

// ---------- main ----------
async function main() {
  const args = parseArgs(process.argv.slice(2));
  const fx = buildFixtures();
  let scenarios = createScenarios(fx);
  if (args.only) {
    const unknown = args.only.filter((n) => !scenarios.some((s) => s.name === n));
    if (unknown.length) throw new Error("없는 시나리오: " + unknown.join(", ") + " (가능: " + scenarios.map((s) => s.name).join(", ") + ")");
    scenarios = scenarios.filter((s) => args.only.includes(s.name));
  }
  fs.mkdirSync(args.out, { recursive: true });

  let server = null;
  let baseUrl = args.url;
  if (!baseUrl) {
    if (!fs.existsSync(path.join(BUILD_DIR, "index.html"))) {
      throw new Error("build/가 없습니다. 먼저 npm run build를 실행하세요(또는 --url로 실행 중인 주소를 주세요).");
    }
    ({ server, url: baseUrl } = await startStaticServer(BUILD_DIR));
    console.log("build/를 " + baseUrl + " 에서 서빙합니다.");
  } else {
    console.log("주소 " + baseUrl + " 를 씁니다.");
  }

  let browser = null;
  const results = [];
  try {
    try {
      browser = await chromium.launch({ channel: "msedge", headless: !args.headed });
    } catch (e) {
      throw new Error("Edge를 띄우지 못했습니다(설치된 Microsoft Edge가 필요합니다): " + String(e.message).split("\n")[0]);
    }
    for (const sc of scenarios) {
      process.stdout.write(sc.name + " … ");
      const r = await runScenario(browser, baseUrl, sc, fx, args.out);
      results.push(r);
      console.log(r.status === "ok" ? "성공" : "실패");
    }
  } finally {
    await browser?.close();
    if (server) {
      server.closeAllConnections?.();
      await new Promise((r) => server.close(() => r()));
    }
  }

  fs.writeFileSync(path.join(args.out, "results.json"), JSON.stringify(results, null, 2));
  writeCompare(args.out, args.design, results);
  printTable(results);
  console.log("비교 화면: " + path.join(args.out, "compare.html"));
  process.exitCode = results.some((r) => r.status !== "ok") ? 1 : 0;
}

main().catch((e) => {
  console.error("하네스 오류: " + (e?.message ?? e));
  process.exitCode = 2;
});
