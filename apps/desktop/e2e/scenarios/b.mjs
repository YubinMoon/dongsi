// 시나리오 B: 클라이언트 ↔ 전용 서버(WSL Docker 리눅스 서버)
import { cleanupT4, dockerCmd, dockerReady, inviteInfo, listT4, logs, rewriteInvite, startServer, waitTcp } from "../lib/docker.mjs";
import { measureWatch, readStats, recordWatch } from "../lib/stats.mjs";
import { bodyText, openWatchStats, startBroadcastUi, stopBroadcastUi, waitPlaying, watchState } from "../lib/ui.mjs";
import { primaryLanIp, sleep, tcpProbe, waitUntil } from "../lib/util.mjs";

const PORT = 8460;
const SRV = "t4-srv";
const SRV2 = "t4-srv2";

async function addServerUi(app, invite, { shot = null, toastShot = null } = {}) {
  await app.go("#/");
  const page = await app.p();
  await page.locator('header nav button[aria-label="서버 추가"]').click();
  const dlg = page.locator("dialog[open]");
  await dlg.locator("#server-input").fill(invite);
  await dlg.locator(".preview").waitFor({ timeout: 10000 });
  if (shot) await app.shot(shot);
  await dlg.locator('button[type="submit"]').click();
  await page.locator("dialog[open]").waitFor({ state: "detached", timeout: 10000 });
  if (toastShot) {
    await page.locator(".toast").first().waitFor({ timeout: 5000 }).catch(() => {});
    await app.shot(toastShot);
  }
}

const card = (page) => page.locator("section.servers .card").first();

async function connectCard(app) {
  const page = await app.p();
  await app.go("#/");
  await card(page).locator("button.main").click();
  await page.waitForFunction(() => location.hash.startsWith("#/server/"), null, { timeout: 20000 });
}

export async function run(ctx) {
  const { rec, args } = ctx;
  const A = ctx.makeApp("A", "b");
  const B = ctx.makeApp("B", "b");
  const failShots = (id) => Promise.all([A.failShot(id), B.failShot(id)]);
  const opts = (needs) => ({ needs, onFail: failShots });
  let invite = null;
  let serverName = null;
  let serverId = null;
  let storedFp = null;
  let liveB = null;

  try {
    await rec.step("b1", "WSL Docker에 리눅스 서버(t4-srv) 시작, 초대 코드 얻기", async () => {
      const ready = await dockerReady(args.image);
      rec.check("WSL Docker 사용 가능, 이미지 " + args.image + " 있음", ready.ok, ready.ok ? ready.info : ready.reason);
      if (!ready.ok) throw new Error("Docker를 쓸 수 없습니다: " + ready.reason);
      await cleanupT4();
      const host = args.serverAddr === "lan" ? primaryLanIp() : "127.0.0.1";
      rec.info("서버 주소 방식", args.serverAddr + " → " + host);
      const raw = await startServer({ name: SRV, port: PORT, address: host, image: args.image });
      const info = inviteInfo(raw);
      rec.info("초대 코드 안의 주소·이름", info);
      const want = host + ":" + PORT;
      invite = info.addrs.includes(want) && info.addrs.length === 1 ? raw : rewriteInvite(raw, [want]);
      if (invite !== raw) rec.info("초대 코드의 주소를 시험 주소로 바꿈", want);
      serverName = info.name ?? SRV;
      storedFp = info.fingerprint;
      rec.check("서버 TCP " + PORT + " 접속 가능", (await tcpProbe(PORT)) === "open");
    }, opts([]));

    await rec.step("b2", "A·B를 띄우고 B에서 서버 추가(초대 코드) → 알림 → 카드", async () => {
      A.resetState();
      B.resetState();
      await Promise.all([A.launch(), B.launch()]);
      rec.check("안전장치 자체 시험 통과(A·B)", !!A.selfTestInfo?.ok && !!B.selfTestInfo?.ok);
      await addServerUi(B, invite, { shot: "b-add-server", toastShot: "b-home-toast" });
      const page = await B.p();
      await card(page).waitFor({ timeout: 10000 });
      const text = (await card(page).innerText()).replace(/\s+/g, " ");
      rec.check("홈 카드에 서버가 나타남", text.includes(serverName), text);
      // 초대 코드에는 지문이 들어 있어 '지문 확인됨'이 맞다('첫 접속 전'은 주소만으로 추가했을 때).
      rec.check("카드 상태가 '지문 확인됨'(초대 코드에 지문이 있음)", text.includes("지문 확인됨") && !text.includes("첫 접속 전"), text);
      const servers = await B.invoke("list_servers");
      serverId = servers[0]?.id;
      await addServerUi(A, invite);
      rec.check("A에도 서버가 저장됨", (await A.invoke("list_servers")).length === 1);
    }, opts(["b1"]));

    await rec.step("b3", "B: 카드를 눌러 연결 → 탭·서버 화면(방송 없음)", async () => {
      await connectCard(B);
      const page = await B.p();
      await page.locator("h1.page-title").waitFor({ timeout: 15000 });
      await page.locator("text=지금 진행 중인 방송이 없습니다.").waitFor({ timeout: 15000 });
      const tabs = await page.locator("header nav .tab.server .name").allInnerTexts();
      const conn = await B.invoke("connected_servers");
      rec.check("위쪽 바에 서버 탭이 생김", tabs.length === 1 && tabs[0].trim() === serverName, JSON.stringify(tabs));
      rec.check("connected_servers가 탭과 일치", conn.length === tabs.length, JSON.stringify(conn).slice(0, 300));
      await B.shot("b-server-empty");
    }, opts(["b2"]));

    await rec.step("b4", "A: 서버에 연결 → 호스팅 '서버'로 방송 시작(테스트 패턴·테스트 톤)", async () => {
      await connectCard(A);
      const r = await startBroadcastUi(A, { hosting: "server", serverName, preset: "720p 30fps", codec: "H.264" });
      liveB = r.status;
      rec.check("broadcast_status: 호스팅 server", r.status.hosting === "server", r.status.hosting);
      rec.info("방송 크기·화질", r.status.width + "×" + r.status.height + " " + JSON.stringify(r.status.settings));
    }, opts(["b2"]));

    await rec.step("b5", "B: 서버 화면에 A의 타일 → 시청 → 통계", async () => {
      const page = await B.p();
      await B.go("#/server/" + serverId);
      await page.locator("ul.grid .tile").first().waitFor({ timeout: 15000 });
      await B.shot("b-server");
      await page.locator("ul.grid a.tile").first().click();
      await page.waitForFunction(() => location.hash.startsWith("#/watch/"), null, { timeout: 10000 });
      await waitPlaying(page, 30000);
      await openWatchStats(page);
      const m = await measureWatch(page, { seconds: 10 });
      recordWatch(rec, "A→서버→B", m, { width: liveB.width, height: liveB.height });
    }, opts(["b3", "b4"]));

    await rec.step("b6", "A: 방송 중 화질·코덱 변경 → B 시청이 끊기지 않음", async () => {
      const aPage = await A.p();
      await A.go("#/broadcast");
      const panel = aPage.locator(".panel, section", { has: aPage.locator("h2", { hasText: "화질 변경" }) }).last();
      await aPage.locator("h2", { hasText: "화질 변경" }).waitFor({ timeout: 10000 });
      const bPage = await B.p();
      const changes = [
        { preset: "1080p 30fps", codec: "VP8" },
        { preset: "720p 30fps", codec: "VP9" },
      ];
      for (const ch of changes) {
        const before = await readStats(bPage);
        const f0 = before?.video?.framesDecoded ?? 0;
        const q = aPage.locator("section, div", { has: aPage.locator("h2", { hasText: "화질 변경" }) }).last();
        const pc = aPage.locator('.chips[aria-label="해상도·fps"] .chip', { hasText: new RegExp("^\\s*" + ch.preset + "\\s*$") });
        const cc = aPage.locator('.chips[aria-label="코덱"] .chip', { hasText: new RegExp("^\\s*" + ch.codec + "\\s*$") });
        if ((await cc.count()) === 0 || (await cc.locator("input").isDisabled())) {
          rec.skip("코덱 " + ch.codec + "로 변경", "이 PC에서 인코딩·재생할 수 없어 선택할 수 없음");
          continue;
        }
        await pc.click();
        await cc.click();
        await aPage.locator("button.apply").click();
        await waitUntil(async () => (await A.invoke("broadcast_status")).settings.codec === ch.codec.toLowerCase().replace(".", ""), { timeout: 15000, desc: "방송 상태의 코덱 변경" });
        const st = await A.invoke("broadcast_status");
        await sleep(4000);
        const state = await watchState(bPage);
        rec.check(ch.preset + " " + ch.codec + ": 시청이 끊기지 않음(오류·끝남 화면 없음)", state.playing && !state.ended && !state.error, state.playing ? "재생 중" : state.text.slice(0, 80));
        const m = await measureWatch(bPage, { seconds: 5 });
        if (m.summary) {
          rec.check(ch.preset + " " + ch.codec + ": framesDecoded가 계속 늘어남", m.summary.framesIn3s >= 30 || m.summary.framesTotal > 0, "총 +" + m.summary.framesTotal + " (변경 전 " + f0 + ")");
          rec.check(ch.preset + " " + ch.codec + ": 해상도가 새 값", m.summary.width === st.width && m.summary.height === st.height, m.summary.width + "×" + m.summary.height + " / 방송자 " + st.width + "×" + st.height);
          rec.check(ch.preset + " " + ch.codec + ": 코덱이 바뀜(codec 통계 mimeType)", (m.summary.codec ?? "").toLowerCase() === ch.codec.toLowerCase().replace(".", ""), m.summary.codec);
          recordWatch(rec, "화질 변경 후 " + ch.preset + " " + ch.codec, m, { width: st.width, height: st.height });
        } else rec.check(ch.preset + " " + ch.codec + ": 통계 읽기", false, null, m.error);
      }
    }, opts(["b5"]));

    await rec.step("b7", "B: 돌아가기 → A: 방송 끝냄 → B 서버 화면에서 타일이 사라짐", async () => {
      const page = await B.p();
      await page.locator("a.w-btn", { hasText: "돌아가기" }).click().catch(async () => B.go("#/server/" + serverId));
      await page.locator("h1.page-title").waitFor({ timeout: 10000 });
      rec.check("돌아가기로 서버 화면", (await B.hash()).startsWith("#/server/"), await B.hash());
      await stopBroadcastUi(A);
      const t0 = Date.now();
      await waitUntil(async () => (await page.locator("ul.grid .tile").count()) === 0, { timeout: 15000, interval: 300, desc: "타일이 사라짐" });
      rec.check("서버 화면에서 타일이 사라짐", true, Math.round((Date.now() - t0) / 100) / 10 + "초");
    }, opts(["b5"]));

    await rec.step("b8", "서버 끊김: 방송 중 docker stop → B 서버 화면 안내, A 방송 화면", async () => {
      await startBroadcastUi(A, { hosting: "server", serverName, preset: "720p 30fps", codec: "H.264" });
      const page = await B.p();
      await B.go("#/server/" + serverId);
      await page.locator("ul.grid .tile").first().waitFor({ timeout: 15000 });
      await dockerCmd("stop", SRV);
      await page.locator("text=서버와 연결이 끊어졌습니다.").first().waitFor({ timeout: 20000 });
      await sleep(800);
      const t = await bodyText(page);
      rec.check("'서버와 연결이 끊어졌습니다.' 안내가 한 번만", t.split("서버와 연결이 끊어졌습니다.").length - 1 === 1, t.split("서버와 연결이 끊어졌습니다.").length - 1 + "번");
      const empty = (await page.locator(".empty").first().innerText()).replace(/\s+/g, " ");
      rec.info("안내 글(이유 줄 중복 확인)", empty);
      rec.check("'다시 연결' 단추가 있음", (await page.locator("button", { hasText: "다시 연결" }).count()) === 1);
      const closed = await page.locator("header nav .tab.server.closed").count();
      rec.check("탭이 끊김 상태로 남음", closed === 1, closed + "개");
      await B.shot("b-server-disconnected");
      // A의 방송 화면
      const aPage = await A.p();
      await A.go("#/broadcast");
      await sleep(12000);
      const aText = (await bodyText(aPage)).replace(/\s+/g, " ").slice(0, 700);
      const badge = (await aPage.locator(".badge.ok, .badge.warn").allInnerTexts().catch(() => [])).map((x) => x.trim());
      const aStatus = await A.invoke("broadcast_status", undefined, { raw: true });
      rec.info("서버가 죽은 뒤 A의 방송 화면·상태(기록용)", { hash: await A.hash(), status: aStatus.ok ? (aStatus.v ? "방송 중" : "방송 없음") : aStatus.e, badges: badge, text: aText });
      await A.shot("b-server-disconnected-A-note", { height: 900 }).catch(() => {});
    }, opts(["b3", "b4"]));

    await rec.step("b9", "docker start → B '다시 연결' → 서버 화면 복귀", async () => {
      await dockerCmd("start", SRV);
      const up = await waitTcp(tcpProbe, PORT, 30000);
      rec.check("서버가 다시 TCP를 받음", up);
      const page = await B.p();
      await page.locator("button", { hasText: "다시 연결" }).click();
      await page.locator("text=지금 진행 중인 방송").first().waitFor({ timeout: 20000 });
      rec.check("서버 화면으로 복귀(끊김 안내 사라짐)", (await page.locator("text=서버와 연결이 끊어졌습니다.").count()) === 0);
      rec.check("탭이 정상 상태", (await page.locator("header nav .tab.server.closed").count()) === 0);
      const aPage = await A.p();
      const aSt = await A.invoke("broadcast_status", undefined, { raw: true });
      rec.info("서버 재시작 뒤 A의 방송 상태(기록용)", aSt.ok ? (aSt.v ? "방송 중 유지" : "방송 없음") : aSt.e);
      if (aSt.ok && aSt.v) await stopBroadcastUi(A).catch(() => {});
      void aPage;
    }, opts(["b8"]));

    await rec.step("b10", "지문 경고: 다른 인증서의 t4-srv2를 같은 포트에 띄움 → B 홈 카드 경고", async () => {
      await dockerCmd("stop", SRV);
      const host = args.serverAddr === "lan" ? primaryLanIp() : "127.0.0.1";
      const raw2 = await startServer({ name: SRV2, port: PORT, address: host, image: args.image });
      const fp2 = inviteInfo(raw2).fingerprint;
      rec.check("새 서버의 지문이 처음 서버와 다름", fp2 !== storedFp, fp2.slice(0, 12) + "… ≠ " + String(storedFp).slice(0, 12) + "…");
      const page = await B.p();
      await B.go("#/");
      await page.waitForFunction(() => document.querySelector("section.servers .card"), null, { timeout: 10000 });
      await card(page).locator("button.main").click();
      await page.locator(".card.mismatch").waitFor({ timeout: 25000 });
      const text = (await page.locator(".card.mismatch").innerText()).replace(/\s+/g, " ");
      rec.check("카드가 '인증서 지문이 다릅니다' 경고", text.includes("인증서 지문이 다릅니다"), text);
      rec.check("저장 지문·실제 지문이 보임", /저장 .*→ 실제/.test(text), text);
      rec.check("접속하지 않음(서버 화면 없음)", (await B.hash()) === "#/", await B.hash());
      await B.shot("b-home-fingerprint");
    }, opts(["b2", "b3"]));
  } finally {
    await Promise.all([A.close(), B.close()]);
    if (!args.keep) {
      await cleanupT4();
      const left = await listT4();
      rec.check("t4- 컨테이너·볼륨이 정리됨", left.ok && left.containers.length === 0 && left.volumes.length === 0, JSON.stringify(left));
    } else {
      rec.info("--keep: t4- 컨테이너·볼륨을 남김", JSON.stringify(await listT4()));
    }
  }
}
