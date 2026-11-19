// 시나리오 shots: A·B 시나리오에서 찍지 못하는 화면(홈 카드 상태, 메뉴·대화 상자, 연결 중·실패, 설정)
import { localTiles, startBroadcastUi, stopBroadcastUi } from "../lib/ui.mjs";
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

const cardOf = (page, name) => page.locator("section.servers .card", { hasText: name });

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
    await rec.step("s1", "앱 띄우고 B에 서버 카드 3개(주소만으로 추가), A가 로컬 랜 방송", async () => {
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

    await rec.step("s2", "b-home: 서버 카드 + 같은 네트워크 방송 타일", async () => {
      await B.go("#/");
      const page = await B.p();
      await localTiles(page).first().waitFor({ timeout: 15000 });
      await sleep(800);
      await B.shot("b-home");
    }, opts(["s1"]));

    await rec.step("s3", "b-server-menu · b-rename · b-remove(열기만)", async () => {
      const page = await B.p();
      const menuBtn = cardOf(page, "친구네 서버").locator('button[aria-haspopup="menu"]');
      await menuBtn.click();
      await page.locator('ul.menu[role="menu"]').waitFor({ timeout: 5000 });
      await B.shot("b-server-menu");
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

    await rec.step("s4", `b-home-connecting(${HANG_PORT}, 응답 없음) · b-home-connect-failed(${CLOSED_PORT}, 닫힘)`, async () => {
      const page = await B.p();
      await B.go("#/");
      await cardOf(page, "응답 없는 서버").locator("button.main").click();
      await page.locator(".card.connecting").waitFor({ timeout: 5000 });
      await sleep(500);
      await B.shot("b-home-connecting");
      await cardOf(page, "꺼진 서버").locator("button.main").click();
      await page.locator(".card.failed").waitFor({ timeout: 20000 });
      const text = (await page.locator(".card.failed").first().innerText()).replace(/\s+/g, " ");
      rec.info("연결 실패 카드 글", text);
      await B.shot("b-home-connect-failed");
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
