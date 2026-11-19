// 시나리오 A: 클라이언트 ↔ 클라이언트(로컬 랜 방송)
import { measureWatch, recordWatch } from "../lib/stats.mjs";
import { bodyText, localTiles, openSetup, openWatchStats, startBroadcastUi, stopBroadcastUi, waitPlaying, watchState } from "../lib/ui.mjs";
import { occupyPort, sleep, tcpProbe, waitUntil } from "../lib/util.mjs";

const PORT_NOTICE = "번 포트를 다른 프로그램";

export async function run(ctx) {
  const { rec } = ctx;
  const A = ctx.makeApp("A", "a");
  const B = ctx.makeApp("B", "a");
  const failShots = (id) => Promise.all([A.failShot(id), B.failShot(id)]);
  const opts = (needs) => ({ needs, onFail: failShots });
  let live1 = null; // 첫 방송의 broadcast_status
  let tileKey = null;

  try {
    await rec.step("a1", "A·B를 띄우고 A 홈이 처음 실행 화면", async () => {
      A.resetState();
      B.resetState();
      await Promise.all([A.launch(), B.launch()]);
      for (const app of [A, B]) {
        rec.check(`${app.label}: 개발 모드 화면(vite ${1420})이 뜸`, app.main.url().startsWith("http://localhost:1420"), app.main.url());
        rec.check(`${app.label}: 안전장치 자체 시험 통과(테스트 패턴만, start_broadcast 차단)`, !!app.selfTestInfo?.ok, JSON.stringify(app.selfTestInfo));
      }
      const t = await A.text();
      rec.check("A: 처음 실행 화면(서버 없음 안내)", t.includes("아직 추가한 서버가 없습니다"));
      await A.shot("b-first-run");
    }, opts([]));

    await rec.step("a2", "A: 로컬 랜으로 테스트 패턴·테스트 톤 방송을 시작", async () => {
      const page = await openSetup(A);
      await A.shot("b-broadcast-setup", { height: 860 });
      const r = await startBroadcastUi(A, { hosting: "lan" });
      live1 = r.status;
      rec.info("방송 설정(시작 직전 화면)", r.setup);
      rec.check("broadcast_status: 로컬 랜, 포트 8450, 비밀번호 없음", live1.hosting === "lan" && live1.port === 8450 && !live1.locked, `hosting=${live1.hosting} port=${live1.port} locked=${live1.locked}`);
      rec.check("broadcast_status: 소스 테스트 패턴, 소리 테스트 톤", /테스트 패턴/.test(live1.sourceName) && live1.audio === "testTone", `${live1.sourceName} / ${live1.audio}`);
      rec.info("방송 크기·화질", `${live1.width}×${live1.height} ${JSON.stringify(live1.settings)}`);
      const notice = await page.locator("p.note", { hasText: PORT_NOTICE }).count();
      rec.check("8450을 점유하지 않았을 때 포트 대체 안내가 없음(DONGSI_PORT≠7884 오탐 확인)", notice === 0, notice ? "안내가 떠 있음(잘못된 안내)" : "없음", "알려진 고장: 다른 작업자가 고치는 중");
      await sleep(2500);
      await A.shot("b-live", { height: 900 });
      rec.check("호스트 시작 TCP 8450 접속 가능", (await tcpProbe(8450)) === "open");
    }, opts(["a1"]));

    await rec.step("a3", "B: 같은 네트워크에 A의 방송 타일이 15초 안에 나타남", async () => {
      await B.go("#/");
      const page = await B.p();
      const t0 = Date.now();
      await localTiles(page).first().waitFor({ timeout: 15000 });
      const dt = Date.now() - t0;
      const tiles = await localTiles(page).evaluateAll((els) => els.map((e) => ({ href: e.getAttribute("href"), text: e.innerText.replace(/\s+/g, " ") })));
      rec.check("B 홈 '같은 네트워크'에 타일이 나타남", tiles.length >= 1, `${dt}ms, ${tiles.length}개`);
      rec.info("타일", tiles);
      rec.check("타일에 경로 표시(LAN/Tailscale/이 PC)", /LAN|Tailscale|이 PC|로컬/i.test(tiles[0].text), tiles[0].text);
      tileKey = tiles[0].href;
      const scan = await B.invoke("scan_local");
      rec.info("scan_local", JSON.stringify(scan).slice(0, 600));
    }, opts(["a2"]));

    await rec.step("a4", "B: 타일을 눌러 시청 → 통계(프레임·해상도·소리·A/V·지연)", async () => {
      const page = await B.p();
      await localTiles(page).first().click();
      await page.waitForFunction(() => location.hash.startsWith("#/watch/"), null, { timeout: 10000 });
      await waitPlaying(page, 30000);
      await openWatchStats(page);
      await sleep(1500);
      await B.shot("b-watch");
      const m = await measureWatch(page, { seconds: 10 });
      recordWatch(rec, "A→B(로컬 랜)", m, { width: live1.width, height: live1.height });
    }, opts(["a3"]));

    await rec.step("a5", "A: 방송 중 화면의 시청자 수가 1", async () => {
      const page = await A.p();
      await waitUntil(async () => (await bodyText(page)).includes("시청자 1명"), { timeout: 10000, desc: "시청자 1명" });
      const st = await A.invoke("broadcast_status");
      rec.check("broadcast_status.viewers === 1", st.viewers === 1, st.viewers);
      rec.check("방송 중 화면 글자가 '시청자 1명'", (await bodyText(page)).includes("시청자 1명"));
    }, opts(["a4"]));

    await rec.step("a6", "B: '새 창으로 보기' → 분리 창 → 창 닫기", async () => {
      const page = await B.p();
      const before = B.popouts().length;
      await page.mouse.move(400, 300);
      await page.locator("button.w-btn", { hasText: "새 창으로 보기" }).click();
      const pop = await waitUntil(() => B.popouts()[0], { timeout: 20000, desc: "분리 창" });
      pop.setDefaultTimeout(10000);
      await B.ensure(pop);
      rec.check("분리 창이 새 CDP 대상으로 생김", B.popouts().length === before + 1, pop.url());
      rec.check("새 창으로 옮기면 원래 화면은 이전 화면(홈)으로 돌아감", (await B.hash()) === "#/", await B.hash());
      await waitPlaying(pop, 30000);
      const st = await watchState(pop);
      rec.info("분리 창 상태", { unmute: st.unmute, playing: st.playing });
      rec.check("분리 창에서 재생 중", st.playing);
      await B.shot("b-watch-popout", { page: pop, width: 1280, height: 760 });
      const m = await measureWatch(pop, { seconds: 5 });
      recordWatch(rec, "A→B 분리 창", m, { width: live1.width, height: live1.height });
      if (st.unmute) {
        await pop.locator("button.unmute").click();
        await sleep(800);
        rec.check("'소리 켜기' 한 번으로 알약이 사라짐", !(await watchState(pop)).unmute);
      } else {
        rec.info("needsUnmute('소리 켜기' 알약)", "기본 설정에서는 재현되지 않음(분리 창이 소리와 함께 자동 재생됨)");
      }
      await pop.locator("button.w-btn", { hasText: "창 닫기" }).click();
      await waitUntil(() => B.popouts().length === before, { timeout: 10000, desc: "분리 창 닫힘" });
      rec.check("창 닫기로 분리 창이 닫힘", B.popouts().length === before);
    }, opts(["a4"]));

    await rec.step("a6b", "B: '소리 켜기'(needsUnmute) 재현 시도 — 자동 재생 정책 인자로 다시 실행하고 사용자 조작 없이 시청 화면으로 이동", async () => {
      const arg = "--autoplay-policy=document-user-activation-required";
      const page0 = await B.p();
      const href = await localTiles(page0).first().getAttribute("href");
      await B.restart({ extraBrowserArgs: [arg] });
      const page = await B.p();
      // 클릭하지 않고 주소만 바꿔(사용자 활성화 없음) 시청 화면을 연다.
      await B.go(href);
      await waitPlaying(page, 30000);
      await sleep(1500);
      const v = await page.evaluate(() => {
        const el = document.querySelector("video");
        return { muted: el.muted, volume: el.volume, paused: el.paused, unmute: !!document.querySelector("button.unmute") };
      });
      rec.info("인자 " + arg + " 로 시청 화면(클릭 없이)", v);
      if (v.unmute) {
        rec.check("'소리 켜기' 알약이 재현됨", true);
        await page.locator("button.unmute").click();
        await sleep(1500);
        const after = await page.evaluate(() => ({ unmute: !!document.querySelector("button.unmute"), muted: document.querySelector("video").muted }));
        rec.check("'소리 켜기' 한 번으로 알약이 사라지고 소리가 켜짐", !after.unmute && !after.muted, JSON.stringify(after));
        const m = await measureWatch(page, { seconds: 4 });
        rec.check("소리를 켠 뒤 오디오 패킷이 늘어남", !!m.summary && m.summary.audioPacketsDelta > 0, m.summary ? "패킷 +" + m.summary.audioPacketsDelta : m.error);
      } else {
        rec.info("needsUnmute 재현 결과", "재현되지 않음: 인자를 주고 사용자 조작 없이 열어도 video.muted=" + v.muted + ", 알약 없음(WebView2가 이 인자를 무시하거나 자동 재생을 허용)");
      }
      await B.go("#/");
    }, opts(["a6"]));

    await rec.step("a7", "A: 방송 끝냄 → B 시청 화면이 '방송이 끝났습니다.' 한 번만", async () => {
      // 분리 창을 닫은 뒤 본 화면은 홈이므로 다시 시청한다.
      const page = await B.p();
      await localTiles(page).first().waitFor({ timeout: 15000 });
      await localTiles(page).first().click();
      await waitPlaying(page, 30000);
      await stopBroadcastUi(A);
      await waitUntil(async () => (await watchState(page)).ended, { timeout: 15000, desc: "B의 '방송이 끝났습니다.'" });
      await sleep(1500);
      const st = await watchState(page);
      rec.check("'방송이 끝났습니다.'가 한 번만 보임", st.endedCount === 1, st.endedCount + "번");
      await B.shot("b-watch-ended");
    }, opts(["a4"]));

    await rec.step("a8", "A: 비밀번호를 걸고 다시 방송 → B: 잠금 타일·비밀번호 폼·오답·정답(기억)", async () => {
      await B.go("#/");
      const r = await startBroadcastUi(A, { hosting: "lan", password: "t4-pass" });
      rec.check("broadcast_status.locked", r.status.locked === true, r.status.locked);
      const page = await B.p();
      await localTiles(page).first().waitFor({ timeout: 15000 });
      const tile = localTiles(page).first();
      await waitUntil(async () => (await tile.locator(".badge.lock").count()) > 0, { timeout: 10000, desc: "타일의 잠금 표시" });
      rec.check("타일에 잠금 표시", true);
      await tile.click();
      await page.locator("form.card").waitFor({ timeout: 15000 });
      rec.check("영상 자리에 비밀번호 폼", (await bodyText(page)).includes("비밀번호가 걸린 방송"));
      await B.shot("b-watch-password");
      await page.locator('form.card input[type="password"]').fill("wrong-pass");
      await page.locator("form.card button.submit").click();
      await page.locator("form.card .err").waitFor({ timeout: 15000 });
      rec.check("틀린 비밀번호 → 인라인 오류", true, (await page.locator("form.card .err").innerText()).trim());
      await page.locator('form.card input[type="password"]').fill("t4-pass");
      await page.locator('form.card input[type="checkbox"]').check();
      await page.locator("form.card button.submit").click();
      await waitPlaying(page, 30000);
      const m = await measureWatch(page, { seconds: 10 });
      recordWatch(rec, "A→B(비밀번호 방송)", m, { width: r.status.width, height: r.status.height });
    }, opts(["a7"]));

    await rec.step("a9", "B: 홈의 '내 서버'에 저장, 감지 타일에 '비밀번호 기억됨'", async () => {
      await B.go("#/");
      const page = await B.p();
      await page.locator("section.servers .card").first().waitFor({ timeout: 15000 });
      const servers = await B.invoke("list_servers");
      rec.check("'내 서버'에 서버가 저장됨", servers.length === 1, servers.map((s) => s.name + " " + (s.addrs ?? []).join(",")).join("; "));
      const savedTile = page.locator("section.local a.tile .saved");
      const t0 = Date.now();
      let shown = await waitUntil(async () => (await savedTile.count()) > 0, { timeout: 12000, interval: 500, desc: "비밀번호 기억됨" }).then(() => true, () => false);
      const scan = await B.invoke("scan_local");
      const rawSaved = scan.flatMap((s) => (s.broadcasts ?? []).map((b) => b.passwordSaved));
      let viaRefresh = false;
      if (!shown) {
        // 자동 갱신으로 안 보이면 '새로 고침'을 눌러 본다(원인 구분용).
        await page.locator("section.local button", { hasText: "새로 고침" }).click().catch(() => {});
        await sleep(3000);
        shown = (await savedTile.count()) > 0;
        viaRefresh = shown;
      }
      rec.check("감지 타일에 '비밀번호 기억됨'", shown, shown ? (viaRefresh ? "새로 고침 뒤에야 보임" : Math.round((Date.now() - t0) / 100) / 10 + "초") : "안 보임", "scan_local의 passwordSaved=" + JSON.stringify(rawSaved));
      await sleep(500);
    }, opts(["a8"]));

    await rec.step("a10", "B: 저장한 비밀번호로 다시 시청 → A: 방송 끝냄 → 시청 끝남·내장 서버 닫힘·타일 사라짐", async () => {
      const page = await B.p();
      await localTiles(page).first().click();
      await page.waitForFunction(() => location.hash.startsWith("#/watch/"), null, { timeout: 10000 });
      await waitPlaying(page, 30000);
      rec.check("저장한 비밀번호로 폼 없이 바로 재생", true);
      await stopBroadcastUi(A);
      await waitUntil(async () => (await tcpProbe(8450)) === "refused", { timeout: 15000, interval: 300, desc: "TCP 8450 거부" });
      rec.check("127.0.0.1:8450 TCP 접속이 거부됨", true);
      await waitUntil(async () => (await watchState(page)).ended, { timeout: 15000, desc: "B의 '방송이 끝났습니다.'" });
      const st = await watchState(page);
      rec.check("B 시청 화면이 '방송이 끝났습니다.' 한 번만", st.endedCount === 1, st.endedCount + "번");
      const t0 = Date.now();
      let gone = false;
      for (let i = 0; i < 12 && !gone; i++) {
        const scan = await B.invoke("scan_local");
        gone = !scan.some((x) => (x.broadcasts ?? []).length > 0 || x.addr?.includes(":8450"));
        if (!gone) await sleep(1000);
      }
      rec.check("B의 scan_local에서 그 서버가 몇 초 안에 없어짐", gone, Math.round((Date.now() - t0) / 100) / 10 + "초");
      await B.go("#/");
      const t1 = Date.now();
      await waitUntil(async () => (await localTiles(page).count()) === 0, { timeout: 15000, interval: 500, desc: "B 홈의 타일이 사라짐" });
      rec.check("B 홈의 감지 타일이 사라짐", true, Math.round((Date.now() - t1) / 100) / 10 + "초");
    }, opts(["a9"]));

    await rec.step("a11", "포트 대체: 8450을 점유하고 방송 → 포트 대체 안내", async () => {
      const occ = await occupyPort(8450);
      try {
        const r = await startBroadcastUi(A, { hosting: "lan" });
        rec.check("8450이 막혀 있으면 다른 포트로 열림", r.status.port !== 8450 && r.status.port !== null, "포트 " + r.status.port);
        const page = await A.p();
        const n = await page.locator("p.note", { hasText: PORT_NOTICE }).count();
        rec.check("포트 대체 안내가 보임", n > 0, n ? "보임" : "없음");
        await sleep(1500);
        await A.shot("b-live-notices", { height: 1000 });
        await stopBroadcastUi(A);
      } finally {
        await occ.release();
      }
    }, opts(["a1"]));
  } finally {
    await Promise.all([A.close(), B.close()]);
  }
}
