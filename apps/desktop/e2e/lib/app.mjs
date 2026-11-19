// 앱 인스턴스 하나: exe를 띄우고, WebView2의 CDP에 playwright-core로 붙어 조작한다.
import fs from "node:fs";
import path from "node:path";
import { chromium } from "playwright-core";
import { GUARD_CHECK, GUARD_SCRIPT, INVOKE_PATCH, INVOKE_RE, SELFTEST_FN } from "./page-scripts.mjs";
import { killTree, spawnLogged } from "./proc.mjs";
import { pngSize, sleep, tcpProbe, waitUntil } from "./util.mjs";

export const DEV_URL_PREFIX = "http://localhost:1420";
const BROWSER_ARGS = [
  "--disable-features=CalculateNativeWinOcclusion",
  "--disable-backgrounding-occluded-windows",
  "--disable-renderer-backgrounding",
];

/** 지울 수 있는 상태 파일 경로. 프로필은 t4a·t4b로 고정이고 경로가 ...\profiles\t4[ab]\state.json 꼴이어야만 쓴다. */
export function stateFilePath(profile) {
  if (!/^t4[ab]$/.test(profile)) throw new Error("프로필 이름은 t4a 또는 t4b여야 합니다: " + profile);
  const appData = process.env.APPDATA;
  if (!appData) throw new Error("APPDATA가 없습니다");
  const p = path.join(appData, "com.dongsi.desktop", "profiles", profile, "state.json");
  if (!/[\\/]com\.dongsi\.desktop[\\/]profiles[\\/]t4[ab][\\/]state\.json$/.test(p)) {
    throw new Error("지울 경로가 예상한 모양이 아닙니다: " + p);
  }
  return p;
}

export class App {
  /**
   * @param {object} o
   * @param {string} o.label "A" | "B"
   * @param {string} o.profile "t4a" | "t4b"
   * @param {number} o.cdpPort
   */
  constructor({ label, profile, cdpPort, exe, rec, outDir, shotsDir, ffmpegBin, port = 8450, scenario = "x" }) {
    Object.assign(this, { label, profile, cdpPort, exe, rec, outDir, shotsDir, ffmpegBin, port, scenario });
    this.proc = null;
    this.browser = null;
    this.context = null;
    this.main = null;
    this.launches = 0;
    this.logPrefix = null;
  }

  /** 처음 실행 화면을 위해 state.json 파일 하나만 지운다(폴더·인증서는 그대로). */
  resetState() {
    const file = stateFilePath(this.profile);
    if (fs.existsSync(file)) fs.unlinkSync(file);
  }

  async launch({ extraBrowserArgs = [] } = {}) {
    if (!fs.existsSync(this.exe)) throw new Error("앱 실행 파일이 없습니다: " + this.exe + " (저장소 루트에서 cargo build -p dongsi-desktop)");
    const probe = await tcpProbe(this.cdpPort);
    if (probe !== "refused") throw new Error(`CDP 포트 ${this.cdpPort}가 이미 쓰이고 있습니다(${probe}). 다른 앱 인스턴스가 떠 있는지 확인하세요.`);
    this.launches++;
    const env = { ...process.env };
    const pathKey = Object.keys(env).find((k) => k.toLowerCase() === "path") ?? "PATH";
    env[pathKey] = this.ffmpegBin + ";" + (env[pathKey] ?? "");
    env.DONGSI_PROFILE = this.profile;
    env.DONGSI_PORT = String(this.port);
    env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = ["--remote-debugging-port=" + this.cdpPort, ...BROWSER_ARGS, ...extraBrowserArgs].join(" ");
    env.RUST_LOG = env.DONGSI_E2E_RUST_LOG ?? "info,dongsi_desktop_lib=debug,dongsi_net=debug,dongsi_server=debug";
    fs.mkdirSync(path.join(this.outDir, "logs"), { recursive: true });
    this.logPrefix = path.join(this.outDir, "logs", `${this.scenario}-${this.label}${this.launches > 1 ? "-" + this.launches : ""}`);
    this.proc = spawnLogged(`앱 ${this.label}(${this.profile})`, this.exe, [], { env, cwd: path.dirname(this.exe), logPrefix: this.logPrefix });
    console.log(`  앱 ${this.label} 시작: pid ${this.proc.pid}, 프로필 ${this.profile}, CDP ${this.cdpPort}`);
    let exitedEarly = null;
    this.proc.exited.then((x) => (exitedEarly = x));

    await waitUntil(
      async () => {
        if (exitedEarly) throw new Error(`앱이 바로 끝났습니다(종료 코드 ${exitedEarly.code}). 로그: ${this.logPrefix}.stderr.log`);
        const r = await fetch(`http://127.0.0.1:${this.cdpPort}/json/version`).catch(() => null);
        return r?.ok;
      },
      { timeout: 60000, interval: 500, desc: `앱 ${this.label}의 CDP(${this.cdpPort})` },
    );
    this.browser = await chromium.connectOverCDP(`http://127.0.0.1:${this.cdpPort}`);
    this.context = this.browser.contexts()[0];
    await this.context.addInitScript(GUARD_SCRIPT);
    // invoke()를 쓰기 불가로 만든 Tauri 대신, vite가 내려 주는 api 모듈을 안전장치를 거치게 패치한다(page-scripts.mjs 참고).
    this.patched = 0;
    await this.context.route(
      (u) => u.href.startsWith(DEV_URL_PREFIX + "/node_modules/.vite/deps/") && /\.js(\?|$)/.test(u.href),
      async (route) => {
        try {
          const resp = await route.fetch();
          let body = await resp.text();
          if (INVOKE_RE.test(body)) {
            body = body.replace(INVOKE_RE, () => INVOKE_PATCH);
            this.patched++;
          }
          await route.fulfill({ response: resp, body });
        } catch (e) {
          await route.abort().catch(() => {});
        }
      },
    );
    this.main = await waitUntil(
      () => this.context.pages().find((p) => p.url().startsWith(DEV_URL_PREFIX) && !p.url().includes("popout=1")),
      { timeout: 60000, interval: 500, desc: `앱 ${this.label}의 화면(${DEV_URL_PREFIX}) — vite dev 서버가 떠 있나요?` },
    );
    this.main.setDefaultTimeout(10000);
    // 패치된 모듈을 읽도록 다시 로드한다.
    await this.main.reload({ waitUntil: "load" });
    await this.settle();
    await this.selfTest(this.main);
  }

  /** 안전장치 자체 시험. 실패하면 던진다(그 상태로는 아무것도 하지 않는다). */
  async selfTest(page = this.main) {
    await this.ensure(page);
    const r = await page.evaluate(`(${SELFTEST_FN})()`);
    if (!r.ok) throw new Error(`앱 ${this.label}: 안전장치 자체 시험 실패 — ${r.reason}. 방송 시험을 중단합니다.`);
    this.selfTestInfo = r;
    return r;
  }

  /** 화면이 다 뜨고(위쪽 바) vite의 의존성 최적화 새로 고침이 끝날 때까지 기다린다. */
  async settle() {
    const page = this.main;
    for (let i = 0; i < 8; i++) {
      await page.waitForLoadState("load", { timeout: 60000 });
      await page.locator("header nav").waitFor({ timeout: 60000 });
      const o1 = await page.evaluate(() => performance.timeOrigin);
      await sleep(2000);
      const o2 = await page.evaluate(() => performance.timeOrigin).catch(() => -1);
      if (o1 === o2) break;
      console.log(`  앱 ${this.label}: 페이지가 새로 로드됨(vite) — 다시 기다립니다`);
    }
    await this.ensure(page);
  }

  /** 안전장치(invoke 감싸기, PC 수집)가 걸려 있는지 확인하고 아니면 건다. */
  async ensure(page = this.main) {
    const ok = await page.evaluate(GUARD_CHECK).catch(() => false);
    if (!ok) {
      await page.evaluate(GUARD_SCRIPT);
      const ok2 = await page.evaluate(GUARD_CHECK);
      if (!ok2) throw new Error(`앱 ${this.label}: 안전장치를 걸지 못했습니다(__TAURI_INTERNALS__ 없음?)`);
    }
  }

  /** 안전장치가 걸린 메인 페이지 */
  async p() {
    await this.ensure(this.main);
    return this.main;
  }

  /** 열려 있는 모든 창(메인 + 분리 창) */
  pages() {
    return this.context.pages().filter((p) => p.url().startsWith(DEV_URL_PREFIX));
  }

  popouts() {
    return this.pages().filter((p) => p.url().includes("popout=1"));
  }

  /** Tauri 명령을 부른다. 오류면 { message, code, ... } 객체를 가진 예외(.cmd = 오류)를 던진다. raw면 { ok, v | e }를 돌려준다. */
  async invoke(cmd, args = undefined, { raw = false, page = null } = {}) {
    const pg = page ?? (await this.p());
    const r = await pg.evaluate(
      async ([c, a]) => {
        try {
          return { ok: true, v: await window.__t4invoke(c, a) };
        } catch (e) {
          return { ok: false, e: e && typeof e === "object" ? e : { message: String(e) } };
        }
      },
      [cmd, args],
    );
    if (raw) return r;
    if (!r.ok) {
      const err = new Error(`${cmd}: ${r.e.message ?? JSON.stringify(r.e)}`);
      err.cmd = r.e;
      throw err;
    }
    return r.v;
  }

  /** 해시 이동(SvelteKit 해시 라우터) */
  async go(hash) {
    const page = await this.p();
    await page.evaluate((h) => {
      location.hash = h;
    }, hash);
  }

  async hash() {
    return (await this.p()).evaluate(() => location.hash);
  }

  async text(page = null) {
    const pg = page ?? (await this.p());
    return pg.locator("body").innerText();
  }

  /**
   * 스크린샷을 shotsDir/<name>.png로 찍는다(CSS 픽셀 크기). height를 주면 그 높이로 잠깐 바꿔 찍고 되돌린다
   * (방송 시작·방송 중 화면은 시안이 720보다 길다). 시안 크기와 다르면 경고로 적는다.
   */
  async shot(name, { height = 720, width = 1100, page = null } = {}) {
    const pg = page ?? (await this.p());
    await pg.evaluate(() => document.fonts.ready);
    fs.mkdirSync(this.shotsDir, { recursive: true });
    const file = path.join(this.shotsDir, name + ".png");
    // 창 크기에 관계없이 시안과 같은 CSS 크기로 찍는다(방송 시작·방송 중 화면은 시안이 720보다 길다).
    const cdp = await this.context.newCDPSession(pg);
    try {
      await cdp.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
      await sleep(600);
      const buf = await pg.screenshot({ scale: "css", animations: "disabled", caret: "hide" });
      fs.writeFileSync(file, buf);
      const size = pngSize(buf);
      this.rec.registerShot(name, { file: path.relative(this.outDir, file).replace(/\\/g, "/"), size, app: this.label });
      const same = size && size.width === width && size.height === height;
      this.rec.add(same ? "pass" : "warn", `스크린샷 ${name}`, size ? size.width + "×" + size.height : "?", same ? null : `시안 크기 ${width}×${height}와 다름`);
    } finally {
      await cdp.send("Emulation.clearDeviceMetricsOverride").catch(() => {});
      await cdp.detach().catch(() => {});
      await sleep(200);
    }
  }

  /** 실패한 단계의 화면(모든 창)을 out/failures/에 저장한다. 오류는 삼킨다. */
  async failShot(tag) {
    try {
      const dir = path.join(this.outDir, "failures");
      fs.mkdirSync(dir, { recursive: true });
      let i = 0;
      for (const pg of this.pages()) {
        const f = path.join(dir, `${this.scenario}-${this.label}-${tag}${i ? "-" + i : ""}.png`);
        fs.writeFileSync(f, await pg.screenshot({ scale: "css", timeout: 8000 }));
        i++;
      }
    } catch {
      // 화면을 찍지 못해도 시험은 계속한다.
    }
  }

  /** 앱 로그(stdout+stderr)에서 패턴에 맞는 줄. */
  logLines(re) {
    const out = [];
    for (const ext of [".stdout.log", ".stderr.log"]) {
      try {
        const text = fs.readFileSync(this.logPrefix + ext, "utf8");
        for (const line of text.split(/\r?\n/)) if (re.test(line)) out.push(line);
      } catch {
        // 아직 없음
      }
    }
    return out;
  }

  async close() {
    try {
      await this.browser?.close();
    } catch {
      // 이미 끊김
    }
    this.browser = null;
    this.context = null;
    this.main = null;
    if (this.proc) {
      await killTree(this.proc.pid);
      await Promise.race([this.proc.exited, sleep(5000)]);
      this.proc = null;
    }
  }

  /** 닫고 다시 띄운다(state.json은 그대로). */
  async restart(opts) {
    await this.close();
    await sleep(1500);
    await this.launch(opts);
  }
}
