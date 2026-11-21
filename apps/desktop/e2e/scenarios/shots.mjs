// 시나리오 shots: A·B 시나리오에서 찍지 못하는 화면(위쪽 탭 상태, 탭 메뉴·대화 상자, 연결 중·실패, 연결 안 된 서버 화면, 설정)
import { liveTiles, openServerTab, serverTab, serverTabNames, serverTabs, startBroadcastUi, stopBroadcastUi, tabMenuSelect, waitTabStatus } from "../lib/ui.mjs";
import { sleep, startHangServer, tcpProbe, waitUntil } from "../lib/util.mjs";


async function addByAddress(app, addr, name) {
  await app.go("#/");
  const page = await app.p();
  await page.locator('header nav button[aria-label="서버 추가"]').click();
  const dlg = page.locator("dialog[open]");
  await dlg.locator("#server-input").fill(addr);
  await dlg.locator("#server-name").waitFor({ timeout: 10000 });
  await dlg.locator("#server-name").fill(name);
  await dlg.locator('button[type="submit"]').click();
  await page.locator("dialog[open]").waitFor({ state: "detached", timeout: 10000 });
}

/** 탭 메뉴 "다시 연결"(사용자가 직접 시도). 이미 연결하는 중이라 항목이 없으면 그대로 둔다. */
const reconnectTab = (page, name) => tabMenuSelect(page, name, "다시 연결");

export async function run(ctx) {
  const { rec } = ctx;
  const { hang: HANG_PORT, closed: CLOSED_PORT, extra: EXTRA_PORT } = ctx.cfg.ports;
  const A = ctx.makeApp("A", "shots");
  const B = ctx.makeApp("B", "shots");
  const failShots = (id) => Promise.all([A.failShot(id), B.failShot(id)]);
  const opts = (needs) => ({ needs, onFail: failShots });
  let hang = null;
  const ids = {};

  try {
    await rec.step("s1", "앱 띄우고 B에 서버 3개(주소만으로 추가, 위쪽 탭 3개), A가 로컬 랜 방송", async () => {
      rec.check(`시험용 포트가 비어 있음(${HANG_PORT}·${CLOSED_PORT}·${EXTRA_PORT})`, [await tcpProbe(HANG_PORT), await tcpProbe(CLOSED_PORT), await tcpProbe(EXTRA_PORT)].every((x) => x === "refused"));
      hang = await startHangServer(HANG_PORT);
      A.resetState();
      B.resetState();
      await Promise.all([A.launch(), B.launch()]);
      await addByAddress(B, "127.0.0.1:" + EXTRA_PORT, "친구네 서버");
      await addByAddress(B, "127.0.0.1:" + HANG_PORT, "응답 없는 서버");
      await addByAddress(B, "127.0.0.1:" + CLOSED_PORT, "꺼진 서버");
      const list = await B.invoke("list_servers");
      for (const s of list) ids[s.name] = s.id;
      rec.check("B에 서버 3개가 저장됨", list.length === 3, list.map((s) => s.name).join(", "));
      await startBroadcastUi(A, { hosting: "lan" });
    }, opts([]));

    await rec.step("s2", "b-home: 방송 중 타일만(서버 카드 없음) + 위쪽 탭(연결 안 된 서버는 흐리게)", async () => {
      await B.go("#/");
      const page = await B.p();
      await liveTiles(page).first().waitFor({ timeout: 15000 });
      rec.check("홈에 서버 카드 구획이 없음", (await page.locator("section.servers").count()) === 0);
      rec.check("위쪽 탭이 저장한 서버 3개", (await serverTabs(page).count()) === 3, (await serverTabNames(page)).join(", "));
      // 아무도 듣지 않는 포트라 자동 연결이 실패해 회색 점(offline)
      const offline = await waitTabStatus(page, "친구네 서버", "offline", 20000).then(() => true, () => false);
      rec.check("'친구네 서버' 탭이 자동 연결 실패(offline)", offline, await serverTab(page, "친구네 서버").getAttribute("data-status"));
      await sleep(800);
      await B.shot("b-home");
    }, opts(["s1"]));

    await rec.step("s3", "b-tab-menu · b-rename · b-remove(열기만)", async () => {
      const page = await B.p();
      const menuBtn = serverTab(page, "친구네 서버").locator('button[aria-haspopup="menu"]');
      await menuBtn.click();
      await page.locator('ul.menu[role="menu"]').waitFor({ timeout: 5000 });
      const items = (await page.locator('[role="menuitem"]').allInnerTexts()).map((t) => t.trim());
      rec.check("연결 안 된 서버의 탭 메뉴: 다시 연결·이름 변경·삭제", items.join(",") === "다시 연결,이름 변경,삭제", items.join(", "));
      await B.shot("b-tab-menu");
      await page.locator('[role="menuitem"]', { hasText: "이름 변경" }).click();
      await page.locator("dialog[open]").waitFor({ timeout: 5000 });
      await B.shot("b-rename");
      await page.locator("dialog[open] button", { hasText: "취소" }).click();
      await page.locator("dialog[open]").waitFor({ state: "detached", timeout: 5000 });
      await menuBtn.click();
      await page.locator('[role="menuitem"]', { hasText: "삭제" }).click();
      await page.locator("dialog[open]").waitFor({ timeout: 5000 });
      await B.shot("b-remove");
      await page.locator("dialog[open] button", { hasText: "취소" }).click();
      await page.locator("dialog[open]").waitFor({ state: "detached", timeout: 5000 });
      rec.check("삭제 대화 상자를 취소해 서버가 그대로 남음", (await B.invoke("list_servers")).length === 3);
    }, opts(["s1"]));

    await rec.step("s4", `b-tabs(연결 중: ${HANG_PORT} 응답 없음, 실패: ${CLOSED_PORT} 닫힘) · b-server-failed · b-server-offline`, async () => {
      const page = await B.p();
      await B.go("#/");
      // 응답 없는 서버: 자동 연결이 아직 기다리는 중이면 이미 연결하는 중, 아니면 탭 메뉴로 다시 시도한다.
      if ((await serverTab(page, "응답 없는 서버").getAttribute("data-status")) !== "connecting") await reconnectTab(page, "응답 없는 서버");
      await waitTabStatus(page, "응답 없는 서버", "connecting", 5000);
      // 꺼진 서버: 사용자가 직접 시도해 실패하면 빨간 점(D-054)
      await waitTabStatus(page, "꺼진 서버", "offline", 20000).catch(() => {});
      await reconnectTab(page, "꺼진 서버");
      await waitTabStatus(page, "꺼진 서버", "failed", 20000);
      rec.info("탭 상태", Object.fromEntries(await Promise.all(["친구네 서버", "응답 없는 서버", "꺼진 서버"].map(async (n) => [n, await serverTab(page, n).getAttribute("data-status")]))));
      await sleep(500);
      await B.shot("b-tabs");
      // 실패한 서버 탭을 누르면 서버 화면에 이유와 "다시 연결"
      await openServerTab(B, "꺼진 서버");
      await page.locator("text=서버에 연결하지 못했습니다.").first().waitFor({ timeout: 10000 });
      rec.info("연결 실패 서버 화면 글", (await page.locator(".empty").first().innerText()).replace(/\s+/g, " "));
      await B.shot("b-server-failed");
      // 자동 연결만 실패한 서버(회색 점)의 서버 화면
      await openServerTab(B, "친구네 서버");
      await page.locator("text=서버에 연결하지 못했습니다.").first().waitFor({ timeout: 20000 });
      await B.shot("b-server-offline");
    }, opts(["s1"]));

    await rec.step("s5", "b-watch-connecting(응답 없는 서버의 시청 화면) · b-watch-error(꺼진 서버, 없는 방송자)", async () => {
      const page = await B.p();
      await B.go("#/watch/" + ids["응답 없는 서버"] + "/no-such-broadcaster");
      await page.locator("text=연결하는 중").first().waitFor({ timeout: 8000 });
      await sleep(500);
      await B.shot("b-watch-connecting");
      await B.go("#/watch/" + ids["꺼진 서버"] + "/no-such-broadcaster");
      await page.locator('.inner[role="alert"]').waitFor({ timeout: 40000 });
      rec.info("시청 오류 화면 글", (await page.locator('.inner[role="alert"]').innerText()).replace(/\s+/g, " "));
      await B.shot("b-watch-error");
    }, opts(["s1"]));

    await rec.step("s6", "b-settings", async () => {
      await B.go("#/settings");
      const page = await B.p();
      await page.locator("h1").first().waitFor({ timeout: 10000 });
      await sleep(800);
      await B.shot("b-settings");
    }, opts(["s1"]));

    await rec.step("s7", "정리: A 방송 끝냄", async () => {
      await stopBroadcastUi(A);
    }, opts(["s1"]));
  } finally {
    await hang?.close();
    await Promise.all([A.close(), B.close()]);
  }
}
