// 화면 조작 도우미(선택자는 apps/desktop/src의 컴포넌트에 맞춘다).
import { sleep, waitUntil } from "./util.mjs";

export const bodyText = (page) => page.locator("body").innerText();

/** 방송 시작 화면을 열고 소스 목록·화질 정보가 다 뜰 때까지 기다린다. */
export async function openSetup(app) {
  const page = await app.p();
  await app.go("#/broadcast");
  await page.locator(".setup h1.page-title").waitFor({ timeout: 15000 });
  await page.locator(".setup .tile").first().waitFor({ timeout: 15000 });
  await page.locator('.chips[aria-label="코덱"] .chip').first().waitFor({ timeout: 15000 });
  // 인코더 정보(비동기)가 들어올 때까지 잠깐
  await waitUntil(async () => !(await bodyText(page)).includes("불러오는 중"), { timeout: 10000, interval: 300, desc: "방송 시작 화면 로딩" }).catch(() => {});
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
      preset: onLabels('.chips[aria-label="해상도·fps"] .chip.on')[0] ?? null,
      codec: onLabels('.chips[aria-label="코덱"] .chip.on')[0] ?? null,
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
export async function startBroadcastUi(app, { hosting, serverName = null, password = "", preset = "720p 30fps", codec = "H.264" }) {
  const page = await openSetup(app);
  await page.locator(".sources .tile", { hasText: "테스트 패턴" }).click();
  await page.locator(".segmented .seg", { hasText: hosting === "lan" ? "로컬 랜" : "추가한 서버" }).click();
  if (hosting === "server") await page.locator('select[aria-label="서버"]').selectOption({ label: serverName });
  await page.locator('.chips[aria-label="해상도·fps"] .chip', { hasText: new RegExp("^\\s*" + preset.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "\\s*$") }).click();
  await page.locator('.chips[aria-label="코덱"] .chip', { hasText: new RegExp("^\\s*" + codec.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "\\s*$") }).click();
  await page.locator('select[id$="-audio"]').selectOption("testTone");
  await page.locator('input[type="password"]').fill(password);
  const setup = await readSetup(page);
  const want = { source: "테스트 패턴", audio: "testTone", preset, codec, password };
  const problems = [];
  if (setup.source !== want.source) problems.push("소스=" + setup.source);
  if (setup.audio !== "testTone") problems.push("소리=" + setup.audio);
  if (setup.preset !== preset) problems.push("화질=" + setup.preset);
  if (setup.codec !== codec) problems.push("코덱=" + setup.codec);
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

/** 같은 네트워크 타일(이름 일부로 찾음) */
export const localTiles = (page) => page.locator("section.local a.tile");

/** 시청 화면 재생 시작까지 기다린다. */
export async function waitPlaying(page, timeout = 30000) {
  await page.locator("video.shown").waitFor({ timeout });
}

/** 시청 화면의 통계 패널을 연다(이미 열려 있으면 그대로). */
export async function openWatchStats(page) {
  await page.mouse.move(400, 300);
  const btn = page.locator("button.w-btn", { hasText: "통계" });
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
