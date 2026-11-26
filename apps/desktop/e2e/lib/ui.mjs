// 화면 조작 도우미(선택자는 apps/desktop/src의 컴포넌트에 맞춘다).
import { readStats } from "./stats.mjs";
import { sleep, waitUntil } from "./util.mjs";

export const bodyText = (page) => page.locator("body").innerText();

const chipText = (text) => new RegExp("^\\s*" + String(text).replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "\\s*$");

/**
 * 화질 이름 "FHD 30fps"·"QHD 60fps"(해상도 칩 FHD·QHD + 주사율 칩 30·60, D-074·D-097·D-101)를 나눈다.
 * width·height·fps는 방송 설정(VideoSettings)에 들어가는 값.
 */
export function parseQuality(label) {
  const m = /^(FHD|QHD) (30|60)(?:fps)?$/.exec(String(label).trim());
  if (!m) throw new Error("화질 이름은 'FHD 30fps' 꼴이어야 합니다(해상도 FHD·QHD + 주사율 30·60): " + label);
  const [width, height] = { FHD: [1920, 1080], QHD: [2560, 1440] }[m[1]];
  return {
    label: m[1] + " " + m[2] + "fps",
    resolution: m[1],
    rate: m[2],
    width,
    height,
    fps: Number(m[2]),
  };
}

/**
 * 해상도·주사율 칩을 고른다(이미 켜진 칩은 누르지 않는다). 코덱 칩은 없다(H.264만, D-103).
 * 방송 중 "화질 변경"에서는 칩을 누를 때마다 바로 적용된다(적용 단추 없음, D-074).
 * 반환: 누른 칩 수
 */
export async function pickQuality(page, quality) {
  const q = parseQuality(quality);
  let clicked = 0;
  for (const [group, text] of [["해상도", q.resolution], ["주사율", q.rate]]) {
    const chip = page.locator(`.chips[aria-label="${group}"] .chip`, { hasText: chipText(text) }).first();
    if (await chip.evaluate((el) => el.classList.contains("on"))) continue;
    await chip.click();
    clicked++;
  }
  return clicked;
}

/** 방송 시작 화면을 열고 소스 목록·화질 정보가 다 뜰 때까지 기다린다. */
export async function openSetup(app) {
  const page = await app.p();
  await app.go("#/broadcast");
  await page.locator(".setup h1.page-title").waitFor({ timeout: 15000 });
  await page.locator(".setup .tile").first().waitFor({ timeout: 15000 });
  await page.locator('.chips[aria-label="해상도"] .chip').first().waitFor({ timeout: 15000 });
  // 소스 목록과 인코더 확인(video_encoder, 처음 부를 때 몇 초 걸릴 수 있다)이 끝날 때까지 잠깐
  await waitUntil(
    async () => {
      const t = await bodyText(page);
      return !t.includes("불러오는 중") && !t.includes("인코더를 확인하는 중");
    },
    { timeout: 20000, interval: 300, desc: "방송 시작 화면 로딩" },
  ).catch(() => {});
  await sleep(400);
  return page;
}

/** 방송 시작 화면의 현재 선택을 읽는다(화면 기준). */
export async function readSetup(page) {
  return page.evaluate(() => {
    const checked = (sel) => {
      const el = document.querySelector(sel);
      return el ? el.textContent.trim().replace(/\s+/g, " ") : null;
    };
    const onLabels = (sel) => [...document.querySelectorAll(sel)].map((e) => e.textContent.trim().replace(/\s+/g, " "));
    const audio = document.querySelector('select[id$="-audio"]');
    const server = document.querySelector('select[aria-label="서버"]');
    return {
      tiles: [...document.querySelectorAll(".sources .tile .name")].map((e) => e.textContent.trim()),
      source: checked(".sources .tile.on .name"),
      hosting: onLabels(".segmented .seg.on")[0] ?? null,
      preset: (() => {
        const r = onLabels('.chips[aria-label="해상도"] .chip.on')[0];
        const f = onLabels('.chips[aria-label="주사율"] .chip.on')[0];
        return r && f ? r + " " + f + "fps" : null;
      })(),
      audio: audio ? audio.value : null,
      server: server ? server.selectedOptions[0]?.textContent.trim() : null,
      password: document.querySelector('input[type="password"]')?.value ?? null,
      summary: checked(".bar .summary .line"),
      error: checked(".bar .summary .error"),
    };
  });
}

/**
 * 방송 설정을 고르고(화면에서 확인한 뒤) 시작한다. 안전장치가 테스트 패턴·테스트 톤이 아니면 막는다.
 * 반환: { setup(시작 직전 화면 선택), status(broadcast_status) }
 */
export async function startBroadcastUi(app, { hosting, serverName = null, password = "", preset = "FHD 30fps" }) {
  const page = await openSetup(app);
  await page.locator(".sources .tile", { hasText: "테스트 패턴" }).click();
  await page.locator(".segmented .seg", { hasText: hosting === "lan" ? "로컬 랜" : "추가한 서버" }).click();
  if (hosting === "server") await page.locator('select[aria-label="서버"]').selectOption({ label: serverName });
  await pickQuality(page, preset);
  await page.locator('select[id$="-audio"]').selectOption("testTone");
  await page.locator('input[type="password"]').fill(password);
  const setup = await readSetup(page);
  const want = { source: "테스트 패턴", audio: "testTone", preset: parseQuality(preset).label, password };
  const problems = [];
  if (setup.source !== want.source) problems.push("소스=" + setup.source);
  if (setup.audio !== "testTone") problems.push("소리=" + setup.audio);
  if (setup.preset !== want.preset) problems.push("화질=" + setup.preset);
  if (setup.password !== password) problems.push("비밀번호 칸");
  if (hosting === "lan" && setup.hosting !== "로컬 랜") problems.push("호스팅=" + setup.hosting);
  if (hosting === "server" && (setup.hosting !== "추가한 서버" || setup.server !== serverName)) problems.push("호스팅/서버=" + setup.hosting + "/" + setup.server);
  if (setup.tiles.length !== 1) problems.push("소스 목록에 테스트 패턴 말고도 있음: " + setup.tiles.join(", "));
  if (problems.length) throw new Error("방송 설정이 화면에서 기대와 다름 — 시작하지 않음: " + problems.join("; "));
  await page.locator("button.start").click();
  await page.locator("h1", { hasText: "방송 중" }).waitFor({ timeout: 40000 });
  const status = await app.invoke("broadcast_status");
  return { setup, status };
}

/** 방송 중 화면에서 "방송 끝내기" → 방송 시작 화면 */
export async function stopBroadcastUi(app) {
  const page = await app.p();
  if (!(await page.locator("h1", { hasText: "방송 중" }).count())) await app.go("#/broadcast");
  await page.locator("button", { hasText: "방송 끝내기" }).click();
  await page.locator(".setup h1.page-title").waitFor({ timeout: 20000 });
}

/** 홈 "방송 중" 타일(내 서버 + 같은 네트워크, D-071) */
export const liveTiles = (page) => page.locator("section.on-air a.tile");

// ---------- 위쪽 바 서버 탭(D-073: 저장한 서버를 저장 순서대로 모두 탭으로, 홈에는 서버 카드가 없다) ----------
// 탭 요소: header nav .tab.server[data-server-id][data-status]. 상태는 connected·connecting·offline(자동 연결 실패)·
// closed(연결돼 있다가 끊김)·failed(사용자가 눌러 실패)·fingerprint(지문 다름)·paused(사용자가 끊음)·idle(시도 전).

/** 서버 탭 전부(저장 순서) */
export const serverTabs = (page) => page.locator("header nav .tab.server");
/** 이름으로 서버 탭 하나 */
export const serverTab = (page, name) => serverTabs(page).filter({ hasText: name }).first();
/** 탭 이름들(화면 읽기용 상태 글자는 빼고) */
export const serverTabNames = async (page) => (await page.locator("header nav .tab.server .label").allInnerTexts()).map((t) => t.trim());

/** 서버 탭의 상태가 `status`가 될 때까지 기다린다. */
export async function waitTabStatus(page, name, status, timeout = 20000) {
  await page.locator(`header nav .tab.server[data-status="${status}"]`, { hasText: name }).first().waitFor({ timeout });
}

/** 서버 탭의 ⋯ 메뉴를 열고 항목을 고른다. 항목이 없으면(예: 연결하는 중이라 "다시 연결"이 없음) 메뉴를 닫고 false. */
export async function tabMenuSelect(page, name, item) {
  await serverTab(page, name).locator('button[aria-haspopup="menu"]').click();
  await page.locator('ul.menu[role="menu"]').waitFor({ timeout: 5000 });
  const entry = page.locator('[role="menuitem"]', { hasText: item });
  if ((await entry.count()) === 0) {
    await page.keyboard.press("Escape");
    return false;
  }
  await entry.first().click();
  return true;
}

/** 서버 탭을 눌러 그 서버 화면으로 간다. `name`이 없으면 첫 서버 탭. 연결되기를 기다리지는 않는다. */
export async function openServerTab(app, name = null) {
  const page = await app.p();
  const tab = name ? serverTab(page, name) : serverTabs(page).first();
  await tab.locator("a.name").click();
  await page.waitForFunction(() => location.hash.startsWith("#/server/"), null, { timeout: 10000 });
  return page;
}

/**
 * 서버 탭을 눌러 서버 화면으로 가고 연결될 때까지 기다린다(앱이 저장한 서버에 자동으로 연결한다, D-071).
 * 연결이 안 돼 있으면 서버 화면의 "다시 연결"을 누른다.
 */
export async function connectServerTab(app, name = null, timeout = 20000) {
  const page = await openServerTab(app, name);
  const live = page.locator("text=지금 방송 중").first();
  const ok = await live.waitFor({ timeout: 5000 }).then(() => true, () => false);
  if (!ok) {
    const retry = page.locator("button", { hasText: "다시 연결" }).first();
    if (await retry.count()) await retry.click().catch(() => {});
    await live.waitFor({ timeout });
  }
  return page;
}

/** 시청 화면 재생 시작까지 기다린다. */
export async function waitPlaying(page, timeout = 30000) {
  await page.locator("video.shown").waitFor({ timeout });
}

/**
 * 시청 화면의 위·아래 막대를 보이게 한다. 재생 중 마우스를 3초 움직이지 않으면 숨고, 같은 자리로 옮기는 것은
 * 움직임으로 세지 않으므로(D-077, src/lib/overlayIdle.ts) 두 자리로 옮긴다.
 */
export async function showWatchOverlay(page) {
  await page.mouse.move(400, 300);
  await page.mouse.move(410, 306);
}

/** 시청 화면의 통계 단추(아이콘만, aria-label "통계", D-077) */
export function watchStatsButton(page) {
  return page.locator('button.w-btn[aria-label="통계"]');
}

/** 시청 화면의 통계 패널을 연다(이미 열려 있으면 그대로). */
export async function openWatchStats(page) {
  await showWatchOverlay(page);
  const btn = watchStatsButton(page);
  if ((await btn.getAttribute("aria-pressed")) !== "true") await btn.click();
  await page.locator(".w-stats, [class*=stats]").first().waitFor({ timeout: 5000 }).catch(() => {});
}

/** 시청 화면 상태(화면 글자 기준) */
export async function watchState(page) {
  return page.evaluate(() => {
    const t = document.body.innerText;
    return {
      playing: !!document.querySelector("video.shown"),
      ended: t.includes("방송이 끝났습니다."),
      endedCount: t.split("방송이 끝났습니다.").length - 1,
      error: !!document.querySelector('.inner[role="alert"]'),
      password: t.includes("비밀번호가 걸린 방송"),
      connecting: t.includes("연결하는 중"),
      unmute: !!document.querySelector("button.unmute"),
      text: t.slice(0, 300),
    };
  });
}

/**
 * 화면 아래 알림(토스트)을 눌러 닫고 모두 사라질 때까지 기다린다. 스크린샷에 알림이 겹쳐 찍히지 않게 한다.
 * (알림은 누르면 닫히고, 안 눌러도 3.5초 뒤 사라진다.) 닫은 개수를 돌려준다.
 */
export async function dismissToasts(page) {
  let closed = 0;
  for (let i = 0; i < 10; i++) {
    const n = await page.evaluate(() => {
      const els = [...document.querySelectorAll(".toasts .toast")];
      els.forEach((e) => e.click());
      return els.length;
    });
    if (n === 0) return closed;
    closed += n;
    await sleep(150);
  }
  return closed;
}

/** 시청 화면의 통계 패널을 닫는다(이미 닫혀 있으면 그대로). */
export async function closeWatchStats(page) {
  await showWatchOverlay(page);
  const btn = watchStatsButton(page);
  if ((await btn.getAttribute("aria-pressed")) === "true") await btn.click();
  await waitUntil(async () => (await page.locator("section.stats").count()) === 0, { timeout: 5000, interval: 100, desc: "통계 패널 닫힘" });
}

/** 통계 패널의 지연·A/V 차이·지터 버퍼·비트레이트가 '-'가 아닌 값으로 채워질 때까지 기다린다. 채워진 값을 돌려준다. */
export async function waitStatsFilled(page, timeout = 20000) {
  const read = () =>
    page.evaluate(() => {
      const out = {};
      document.querySelectorAll("section.stats dt").forEach((dt) => {
        out[dt.textContent.trim()] = dt.nextElementSibling?.textContent.trim() ?? "";
      });
      return out;
    });
  const need = ["비트레이트", "지연", "A/V 차이", "지터 버퍼"];
  const empty = (v) => !v || /(^|[ /+])[-–—](?![0-9])/.test(v);
  let last = {};
  await waitUntil(
    async () => {
      last = await read();
      return need.every((k) => k in last && !empty(last[k]));
    },
    { timeout, interval: 400, desc: "통계 값(지연·A/V·지터 버퍼·비트레이트)이 채워짐: " + JSON.stringify(last) },
  );
  return last;
}

/** 영상이 실제로 재생 중인지(framesDecoded가 1초 사이에 minFrames 이상 늘어남) 확인한다. */
export async function waitFramesAdvancing(page, { minFrames = 15, timeout = 20000 } = {}) {
  let delta = 0;
  await waitUntil(
    async () => {
      // 페이지가 다시 불러오는 순간 evaluate가 실패할 수 있어 그 경우는 다음 시도로 넘긴다.
      const a = await readStats(page).catch(() => null);
      await sleep(1000);
      const b = await readStats(page).catch(() => null);
      delta = (b?.video?.framesDecoded ?? 0) - (a?.video?.framesDecoded ?? 0);
      return delta >= minFrames;
    },
    { timeout, interval: 100, desc: "framesDecoded 증가(" + minFrames + "프레임/초 이상), 마지막 증가량 " + delta },
  );
  return delta;
}
