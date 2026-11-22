// 화면 상태 하나당 시나리오 하나. 이름(b-xxx)은 예전 B 디자인 시안(지금은 지움)의 화면 이름에서 왔다.
//
// 시나리오 모양:
//   {
//     name, viewport: { width, height }, hash: "#/...",
//     mock:  { 명령: 항목 },     // 기본 표(baseTable) 위에 덮어쓴다. 항목 종류는 mock-ipc.js 머리말 참고.
//     state: { ... },            // fn 항목이 ctx.state로 읽고 쓰는 값
//     steps: [ ... ],            // 할 동작(아래 "동작"). 맨 앞의 waitFor가 화면이 떴는지 확인한다.
//     note:  "무엇을 찍는지",
//   }
//
// 대상(target) — 하네스 계약의 이름만 쓴다(화면 구현 방식에 기대지 않는다):
//   { role, name, exact? }     접근 가능한 역할 + 이름(기본은 부분 일치)
//   { label, exact? }          입력란 label
//   { text, exact? }           글자
//   { css }                    CSS 선택자
//   { card: id }               위쪽 바 서버 탭(data-server-id, D-073). 이름은 홈 서버 카드 때 그대로다(shots.mjs).
//   { cardButton: id, name }   서버 탭 안 링크(⋯ 메뉴 단추 제외, 접근 이름에 서버 이름). 누르면 서버 화면
//   { cardMenu: name }         서버 탭 ⋯ 단추(aria-label "'이름' 서버 메뉴")
//   아래의 tab()·tabIn()·tabLink()·tabMenu()가 이것들을 탭 이름으로 감싼다.
//   { menuitem: "글자" }       role=menuitem
//   { tile: "방송자 이름" }    방송 타일 링크
//   { or: [대상, 대상] }       앞의 것이 없으면 뒤의 것
//
// 동작: { waitFor: 대상 } { click: 대상 } { fill: 대상, value } { press: "Enter" } { hover: 대상 }
//       { wait: ms } { emit: [이벤트, 페이로드] } { hash: "#/..." } { eval: "js 소스" }
// (함수 항목 fn(...)은 직렬화되어 브라우저에서 실행되므로 바깥 변수를 쓸 수 없다. 필요한 값은 data로 넘긴다.)

import { buildFixtures } from "./fixtures.mjs";

export const value = (v, delay) => ({ __mock: "value", value: v, delay });
export const reject = (v, delay) => ({ __mock: "error", value: v, delay });
export const pending = () => ({ __mock: "pending" });
/** f는 (args, ctx, data) => 값. throw하면 거절. */
export const fn = (f, data) => ({ __mock: "fn", source: f.toString(), data });

/**
 * connect_server: 서버마다 다르게. `overrides`의 key는 { pending: true }(끝나지 않음) 또는 { error }(거절),
 * 나머지는 fx.live에 있으면 성공. 앱이 저장한 서버 모두에 자동으로 연결하므로(D-071) 한 서버만 실패시킬 때 쓴다.
 * `ctx.state.down`(서버 key 배열)에 든 서버는 꺼진 것으로 보고 거절한다(시나리오가 { eval }로 넣는다).
 */
const connectPerServer = (live, overrides) =>
  fn(
    (args, ctx, d) => {
      const o = d.overrides[args.key];
      if (o && o.pending) return new Promise(() => {});
      if (o) throw o.error;
      if ((ctx.state.down || []).includes(args.key)) {
        throw { message: "127.0.0.1:7884에 접속하지 못했습니다: 대상 컴퓨터에서 연결을 거부했으므로 연결하지 못했습니다. (os error 10061)" };
      }
      if (!d.live[args.key]) throw { message: "서버를 찾을 수 없습니다" };
      // connected_servers가 이 연결을 알게 한다(실제 백엔드처럼).
      ctx.state.connectedNow = [...new Set([...(ctx.state.connectedNow || []), args.key])];
      ctx.state.disconnected = (ctx.state.disconnected || []).filter((k) => k !== args.key);
      return d.live[args.key];
    },
    { live, overrides },
  );

/** 거의 모든 화면이 부르는 명령의 기본 표. 시나리오가 덮어쓴다. */
function baseTable(fx, { connected }) {
  const { home, minsu, corp } = fx.servers;
  return {
    get_settings: fx.settings,
    list_servers: [home, minsu, corp],
    scan_local: fx.localServers,
    // 시나리오의 connected(앱을 켜기 전부터 연결돼 있던 서버) + 앱이 connect_server로 연결한 서버 − 끊은·꺼진 서버
    connected_servers: fn(
      (_a, ctx, ids) =>
        [...new Set([...ids, ...(ctx.state.connectedNow || [])])].filter(
          (k) => !(ctx.state.down || []).includes(k) && !(ctx.state.disconnected || []).includes(k),
        ),
      connected.map((s) => s.id),
    ),
    // 끊은 서버는 connected_servers에서 뺀다(실제 백엔드처럼). 다시 연결하면 connect_server가 되살린다.
    disconnect_server: fn((args, ctx) => {
      ctx.state.connectedNow = (ctx.state.connectedNow || []).filter((k) => k !== args.key);
      ctx.state.disconnected = [...(ctx.state.disconnected || []), args.key];
      return null;
    }),
    connect_server: connectPerServer(fx.live, {}),
    broadcast_status: null,
    broadcast_preview: null,
    list_capture_sources: fx.sources,
    capture_source_thumbnails: fx.thumbnails,
    // 방송 타일 썸네일(D-076). 그림이 없는 방송은 null(자리 표시).
    broadcast_thumbnail: fn((args, _ctx, pics) => pics[args.key + "/" + args.broadcasterId] ?? null, fx.broadcastThumbnails),
    available_encoders: fx.encoders,
    stop_watch: null,
    request_keyframe: null,
    // 시청자의 서버 경로(Wi-Fi 감지, D-088). 통계 패널의 "네트워크" 줄
    network_path: { kind: "wifi", viaTunnel: false, signalQuality: 78, rssiDbm: -58, rxRateMbps: 866, channel: 36, streamingMode: true },
  };
}

export function createScenarios(fx = buildFixtures()) {
  const { home, minsu, corp, friend, study, club } = fx.servers;
  const { harin, doyun } = fx.broadcasts;
  const two = [home, minsu]; // 앱을 켜기 전부터 "우리집 서버", "민수 PC"가 연결돼 있는 시안
  const DEFAULT = { width: 1100, height: 720 };

  /** @returns {object} 시나리오(기본 값을 채운다) */
  const s = (name, o) => ({
    viewport: DEFAULT,
    hash: "#/",
    connected: two,
    mock: {},
    state: {},
    steps: [],
    ...o,
    name,
    table: { ...baseTable(fx, { connected: o.connected ?? two }), ...(o.mock ?? {}) },
  });

  // 위쪽 바 서버 탭(D-073: 저장한 서버를 모두 탭으로). 대상 이름은 shots.mjs의 card·cardButton·cardMenu를 쓴다.
  /** 서버 탭 */
  const tab = (server) => ({ card: server.id });
  /** 상태(data-status)가 `status`인 서버 탭: connected·connecting·offline·closed·failed·fingerprint·paused·idle */
  const tabIn = (server, status) => ({ css: '[data-server-id="' + server.id + '"][data-status="' + status + '"]' });
  /** 서버 탭의 링크(누르면 서버 화면) */
  const tabLink = (server) => ({ cardButton: server.id, name: server.name });
  /** 서버 탭의 ⋯ 단추 */
  const tabMenu = (server) => ({ cardMenu: server.name });
  /** 서버 탭을 오른쪽 클릭한다(이름 위, 탭 왼쪽에서 40px). */
  const rightClickTab = (server) => ({
    eval:
      "(() => { const a = document.querySelector('[data-server-id=\"" +
      server.id +
      "\"] a'); const r = a.getBoundingClientRect();" +
      " a.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 40, clientY: r.bottom - 6 })); })()",
  });

  /** 홈 화면이 떴는지: 우리집 서버 탭과 "방송 중"의 같은 네트워크 타일 */
  const homeReady = [{ waitFor: tab(home) }, { waitFor: { text: "준호" } }];

  const fingerprintError = (stored, actual, storedShort, actualShort) => ({
    message:
      "서버 인증서 지문이 저장된 것과 다릅니다(저장 " +
      storedShort +
      "…, 받은 값 " +
      actualShort +
      "…). 서버를 다시 설치했다면 목록에서 삭제하고 새 초대 코드로 추가해 주세요.",
    code: "fingerprintMismatch",
    detail: { stored, actual },
  });
  const homeFingerprintError = fingerprintError(fx.fingerprints.home, fx.fingerprints.actual, "3A:9F:12:C4", "B7:0E:5D:1C");
  const corpRefused = {
    message:
      "100.88.12.7:7900에 접속하지 못했습니다: 대상 컴퓨터에서 연결을 거부했으므로 연결하지 못했습니다. (os error 10061)",
  };

  /** b-tabs: 탭 다섯 개의 상태가 모두 다르다(연결됨·연결 중·자동 연결 실패·사용자가 눌러 실패·지문 다름). */
  const mixedTabs = {
    connected: [home],
    mock: {
      list_servers: [home, minsu, corp, study, club],
      connect_server: connectPerServer(fx.live, {
        [minsu.id]: { pending: true },
        [corp.id]: { error: corpRefused },
        [study.id]: {
          error: {
            message:
              "study.example.org:7884에 접속하지 못했습니다: 연결된 구성원으로부터 응답이 없어 연결하지 못했습니다. (os error 10060)",
          },
        },
        [club.id]: { error: fingerprintError(fx.fingerprints.club, fx.fingerprints.clubActual, "47:A0:9C:E5", "C2:5B:81:F4") },
      }),
    },
    steps: [
      ...homeReady,
      { waitFor: tabIn(corp, "offline") },
      { waitFor: tabIn(club, "fingerprint") },
      { waitFor: tabIn(study, "offline") },
      // 스터디 서버는 사용자가 탭 메뉴 "다시 연결"로 시도해 실패했다(빨간 점, D-054)
      { click: tabMenu(study) },
      { click: { menuitem: "다시 연결" } },
      { waitFor: tabIn(study, "failed") },
      { waitFor: tabIn(minsu, "connecting") },
    ],
  };

  const openAddServer = [
    { click: { role: "button", name: "서버 추가" } },
    {
      fill: { or: [{ label: "주소 또는 초대 코드" }, { css: "[role=dialog] input, dialog input" }] },
      value: fx.sampleInvite,
    },
    { waitFor: { text: "방송 초대 코드" } },
  ];

  const watchHash = (b) => "#/watch/" + home.id + "/" + b.broadcasterId;
  /** 시청 화면: connect_server는 성공(방송 정보를 읽는다), watch_broadcast만 시나리오마다 다르다. */
  const watchMock = (watchBroadcast) => ({ watch_broadcast: watchBroadcast });
  /**
   * 재생 중 시청 화면(b-watch-playing*, D-077): 하네스에는 실제 미디어가 없으므로 watch_broadcast가 이 페이지의
   * RTCPeerConnection을 흉내 낸다. offer는 진짜로 만들고, setRemoteDescription은 바로 끝내며 캔버스 무늬 영상 트랙을 주고
   * "connected"를 알린다. getStats는 고정 통계(1920×1080 60fps H264, 지연 84ms, A/V 12ms, 손실·복구·멈춤 작은 값)를 돌려준다.
   * Date는 멈춰 있으므로(shots.mjs) 시각은 performance.now()로 센다. 실제 재생 화면은 여전히 e2e(실제 앱)에서 찍는다.
   */
  const watchPlaying = fn(() => {
    const P = window.RTCPeerConnection.prototype;
    if (!P.__fakePlaying) {
      P.__fakePlaying = true;
      P.setRemoteDescription = async function () {
        const pc = this;
        const canvas = document.createElement("canvas");
        canvas.width = 1280;
        canvas.height = 720;
        const g = canvas.getContext("2d");
        // 테스트 패턴처럼 어두운 색 막대 일곱 개와 움직이는 상자
        const bars = ["#b4b4b4", "#b4b400", "#00b4b4", "#00b400", "#b400b4", "#b40000", "#0000b4"];
        const draw = () => {
          bars.forEach((c, i) => {
            g.fillStyle = c;
            g.fillRect((i * 1280) / 7, 0, 1280 / 7 + 1, 560);
          });
          g.fillStyle = "#101114";
          g.fillRect(0, 560, 1280, 160);
          const x = ((performance.now() / 8) % 1180) | 0;
          g.fillStyle = "#f3f4f6";
          g.fillRect(x, 600, 100, 80);
        };
        draw();
        setInterval(draw, 100);
        const stream = canvas.captureStream(30);
        const track = stream.getVideoTracks()[0];
        Object.defineProperty(pc, "connectionState", { value: "connected", configurable: true });
        setTimeout(() => {
          if (pc.ontrack) pc.ontrack({ track, streams: [stream], receiver: { track } });
          if (pc.onconnectionstatechange) pc.onconnectionstatechange(new Event("connectionstatechange"));
        }, 50);
      };
      P.getStats = async function () {
        const ms = performance.now();
        const t = 1.7e12 + ms;
        const NTP = 2208988800000;
        const frames = Math.floor(ms / 16.67);
        const samples = Math.floor(ms * 48);
        const list = [
          {
            id: "V",
            type: "inbound-rtp",
            kind: "video",
            timestamp: t,
            codecId: "C",
            frameWidth: 1920,
            frameHeight: 1080,
            framesPerSecond: 60,
            decoderImplementation: "ExternalDecoder",
            powerEfficientDecoder: true,
            bytesReceived: Math.round(ms * 855),
            framesDecoded: frames,
            jitterBufferDelay: frames * 0.038,
            jitterBufferEmittedCount: frames,
            estimatedPlayoutTimestamp: t - 84 + NTP,
            // 진단 통계(D-088): 그럴듯한 작은 누적 값
            packetsLost: 12,
            packetsReceived: Math.floor(ms / 1.5),
            nackCount: 9,
            pliCount: 1,
            retransmittedPacketsReceived: 8,
            keyFramesDecoded: 3,
            framesDropped: 3,
            freezeCount: 1,
            totalFreezesDuration: 0.4,
            totalDecodeTime: frames * 0.0031,
            packetsDiscarded: 0,
            jitter: 0.004,
          },
          {
            id: "A",
            type: "inbound-rtp",
            kind: "audio",
            timestamp: t,
            bytesReceived: Math.round(ms * 16),
            packetsReceived: Math.floor(ms / 20),
            jitterBufferDelay: samples * 0.042,
            jitterBufferEmittedCount: samples,
            estimatedPlayoutTimestamp: t - 96 + NTP,
            concealedSamples: 0,
            packetsLost: 2,
            packetsDiscarded: 0,
            concealmentEvents: 3,
            jitter: 0.002,
            codecId: "CA",
          },
          { id: "C", type: "codec", timestamp: t, mimeType: "video/H264" },
          { id: "CA", type: "codec", timestamp: t, mimeType: "audio/opus" },
          { id: "P", type: "candidate-pair", timestamp: t, nominated: true, state: "succeeded", currentRoundTripTime: 0.003 },
        ];
        return new Map(list.map((s) => [s.id, s]));
      };
    }
    return { session: 1, answer: "v=0\r\n", network: { kind: "wifi", viaTunnel: false, signalQuality: 78, rssiDbm: -58, rxRateMbps: 866, channel: 36, streamingMode: true } };
  });
  /** 재생이 시작되고 영상 위로 마우스를 옮긴 뒤(막대가 보인 채로 3초를 새로 잰다) */
  const watchPlayingReady = [{ waitFor: { css: "video.shown" } }, { hover: { css: "video" } }];

  return [
    // ---------- 홈(방송 목록만, D-073)·위쪽 바 탭 ----------
    s("b-home", {
      note: "서버 3개(모두 자동 연결, 탭 3개), 방송 중: 우리집 서버 3개 + 같은 네트워크 2개. 홈에 서버 카드 없음",
      steps: [...homeReady, { waitFor: { tile: "하린" } }, { waitFor: tabIn(corp, "connected") }],
    }),
    s("b-tabs", {
      note: "탭 상태 섞임: 우리집 서버 연결됨, 민수 PC 연결하는 중(스피너), 회사 테일넷 자동 연결 실패(회색 점), 스터디 서버 눌러서 실패(빨간 점), 동아리 서버 지문 다름(호박색 경고)",
      ...mixedTabs,
    }),
    s("b-tabs-narrow", {
      note: "b-tabs를 좁은 창(760px)에서: 탭이 넘치면 가로 스크롤",
      viewport: { width: 760, height: 560 },
      ...mixedTabs,
    }),
    s("b-first-run", {
      note: "서버 0개(탭 없음), 방송 중 없음(찾는 중…)",
      connected: [],
      mock: { list_servers: [], scan_local: [], connected_servers: [] },
      steps: [{ wait: 600 }],
    }),
    s("b-tab-menu", {
      note: "연결된 우리집 서버 탭의 ⋯ 메뉴: 연결 끊기·이름 변경·삭제",
      steps: [...homeReady, { waitFor: tabIn(home, "connected") }, { click: tabMenu(home) }, { waitFor: { menuitem: "연결 끊기" } }],
    }),
    s("b-tab-menu-offline", {
      note: "자동 연결에 실패한 회사 테일넷 탭을 오른쪽 클릭: 그 자리에 메뉴(다시 연결·이름 변경·삭제)",
      mock: { connect_server: connectPerServer(fx.live, { [corp.id]: { error: corpRefused } }) },
      steps: [...homeReady, { waitFor: tabIn(corp, "offline") }, rightClickTab(corp), { waitFor: { menuitem: "다시 연결" } }],
    }),
    s("b-rename", {
      note: "회사 테일넷 탭 메뉴 → 이름 변경 대화 상자(입력란에 현재 이름)",
      steps: [
        ...homeReady,
        { click: tabMenu(corp) },
        { click: { menuitem: "이름 변경" } },
        { waitFor: { role: "dialog" } },
        { wait: 200 },
      ],
    }),
    s("b-remove", {
      note: "회사 테일넷 탭 메뉴 → 삭제 확인 대화 상자",
      steps: [
        ...homeReady,
        { click: tabMenu(corp) },
        { click: { menuitem: "삭제" } },
        { waitFor: { role: "dialog" } },
        { wait: 200 },
      ],
    }),
    s("b-home-toast", {
      note: "초대 코드로 '친구의 서버'를 추가한 직후(알림이 떠 있음, 탭 4개)",
      state: { servers: [home, minsu, corp] },
      mock: {
        list_servers: fn((_a, ctx) => ctx.state.servers),
        // 추가한 서버에도 자동으로 연결된다(D-071).
        connect_server: connectPerServer(
          {
            ...fx.live,
            [friend.id]: {
              key: friend.id,
              serverName: friend.name,
              canPublish: true,
              hostOnly: false,
              fingerprint: friend.fingerprint,
              broadcasts: [],
            },
          },
          {},
        ),
        parse_server_input: fx.parsedInvite,
        add_server: fn((_a, ctx, data) => {
          ctx.state.servers.push(data.server);
          return data;
        }, fx.addResult),
      },
      steps: [
        ...homeReady,
        ...openAddServer,
        { click: { role: "button", name: "추가", exact: true } },
        { waitFor: { text: "서버를 추가했습니다" } },
        { waitFor: { card: friend.id } },
        { wait: 300 },
      ],
    }),
    s("b-add-server", {
      note: "서버 추가 대화 상자: 방송 초대 코드를 붙여 넣어 해석된 상태",
      mock: { parse_server_input: fx.parsedInvite },
      steps: [...homeReady, ...openAddServer, { wait: 200 }],
    }),

    // ---------- 서버 ----------
    s("b-server", {
      note: "우리집 서버 화면: 방송 3개(하린·도윤·서연)",
      hash: "#/server/" + home.id,
      steps: [
        { waitFor: { tile: "하린" } },
        { waitFor: { tile: "도윤" } },
        { waitFor: { tile: "서연" } },
      ],
    }),
    s("b-server-empty", {
      note: "회사 테일넷 서버 화면: 방송 없음(탭 3개)",
      hash: "#/server/" + corp.id,
      connected: [home, minsu, corp],
      steps: [{ waitFor: { text: "진행 중인 방송이 없습니다" } }],
    }),
    s("b-server-disconnected", {
      note: "우리집 서버 화면을 띄운 뒤 server-closed로 끊김. 2초 뒤 자동 재시도도 실패해 '끊어졌습니다'가 그대로(탭은 회색 점)",
      hash: "#/server/" + home.id,
      steps: [
        { waitFor: { tile: "하린" } },
        { eval: "window.__mock.state.down = [" + JSON.stringify(home.id) + "]" },
        { emit: ["server-closed", { key: home.id, reason: "서버가 닫혔습니다." }] },
        { waitFor: { text: "연결이 끊어졌습니다" } },
        { waitFor: tabIn(home, "closed") },
        { wait: 2600 },
        { waitFor: { text: "연결이 끊어졌습니다" } },
      ],
    }),
    s("b-server-offline", {
      note: "자동 연결에 실패한 회사 테일넷 탭을 눌러 들어간 서버 화면: 이유 + 다시 연결(D-073)",
      mock: { connect_server: connectPerServer(fx.live, { [corp.id]: { error: corpRefused } }) },
      steps: [
        ...homeReady,
        { waitFor: tabIn(corp, "offline") },
        { click: tabLink(corp) },
        { waitFor: { text: "서버에 연결하지 못했습니다" } },
        { wait: 300 },
      ],
    }),
    s("b-server-failed", {
      note: "회사 테일넷 서버 화면에서 '다시 연결'을 눌러 또 실패: 빨간 안내, 탭은 빨간 점",
      hash: "#/server/" + corp.id,
      mock: { connect_server: connectPerServer(fx.live, { [corp.id]: { error: corpRefused } }) },
      steps: [
        { waitFor: { text: "서버에 연결하지 못했습니다" } },
        { click: { role: "button", name: "다시 연결" } },
        { waitFor: tabIn(corp, "failed") },
        { wait: 200 },
      ],
    }),
    s("b-server-fingerprint", {
      note: "우리집 서버: 인증서 지문 불일치(자동 연결에서 바로 보임). 탭은 호박색 경고, 서버 화면은 저장·실제 지문과 다시 연결",
      hash: "#/server/" + home.id,
      connected: [minsu],
      mock: { connect_server: connectPerServer(fx.live, { [home.id]: { error: homeFingerprintError } }) },
      steps: [{ waitFor: { text: "인증서 지문이 다릅니다" } }, { waitFor: tabIn(home, "fingerprint") }, { wait: 200 }],
    }),
    s("b-server-connecting", {
      note: "회사 테일넷 서버 화면: connect_server가 끝나지 않는 '연결하는 중'(탭은 스피너)",
      hash: "#/server/" + corp.id,
      mock: { connect_server: connectPerServer(fx.live, { [corp.id]: { pending: true } }) },
      steps: [{ waitFor: { text: "서버에 연결하는 중" } }, { waitFor: tabIn(corp, "connecting") }, { wait: 200 }],
    }),
    s("b-server-paused", {
      note: "우리집 서버 화면에서 탭 ⋯ 메뉴 '연결 끊기' 뒤: 탭은 흐리게 남고 화면은 '연결을 끊었습니다' + 다시 연결(서버 화면에는 ⋯ 메뉴 없음, D-079)",
      hash: "#/server/" + home.id,
      steps: [
        { waitFor: { tile: "하린" } },
        { click: tabMenu(home) },
        { click: { menuitem: "연결 끊기" } },
        { waitFor: { text: "연결을 끊었습니다" } },
        { waitFor: tabIn(home, "paused") },
        { wait: 200 },
      ],
    }),

    // ---------- 설정 ----------
    s("b-settings", {
      note: "설정: 표시 이름 지우, 설치 ID, 버전 0.1.0, 라이선스·FFmpeg(LGPL 고지, 소스 위치 복사)·제3자 고지 행",
      hash: "#/settings",
      steps: [{ waitFor: { label: "표시 이름" } }, { waitFor: { text: "LGPL 2.1 이상" } }, { wait: 200 }],
    }),
    s("b-settings-licenses", {
      note: "설정 > 제3자 고지(#/settings/licenses): THIRD_PARTY_NOTICES.md를 고정폭·스크롤로 보여 줌(시안 없음)",
      hash: "#/settings/licenses",
      steps: [{ waitFor: { css: "pre.notices" } }, { waitFor: { text: "제3자 고지", exact: true } }, { wait: 300 }],
    }),

    // ---------- 시청 상태 화면(재생 화면은 아래 b-watch-playing*: 가짜 미디어. 실제 재생 화면 b-watch·b-watch-popout은 e2e에서 찍는다) ----------
    s("b-watch-password", {
      note: "도윤 방송: 영상 자리의 비밀번호 입력 폼",
      hash: watchHash(doyun),
      mock: watchMock(
        reject({ message: "비밀번호가 필요한 방송입니다", code: "passwordRequired" }),
      ),
      steps: [{ waitFor: { label: "'도윤' 방송의 비밀번호" } }, { wait: 200 }],
    }),
    s("b-watch-connecting", {
      note: "하린 방송: watch_broadcast가 끝나지 않는 '연결하는 중' 상태",
      hash: watchHash(harin),
      mock: watchMock(pending()),
      steps: [{ waitFor: { text: "연결하는 중" } }, { wait: 200 }],
    }),
    s("b-watch-ended", {
      note: "하린 방송: 연결하는 중에 broadcast-ended 이벤트가 와서 끝난 상태",
      hash: watchHash(harin),
      mock: watchMock(pending()),
      steps: [
        { waitFor: { text: "연결하는 중" } },
        { emit: ["broadcast-ended", { key: home.id, broadcasterId: harin.broadcasterId }] },
        { waitFor: { text: "방송이 끝났습니다" } },
        { wait: 200 },
      ],
    }),
    s("b-watch-error", {
      note: "하린 방송: 미디어 연결 실패 오류",
      hash: watchHash(harin),
      mock: watchMock(
        reject({
          message: "미디어 연결에 실패했습니다. 방화벽에서 UDP가 막혀 있는지 확인해 주세요.",
        }),
      ),
      steps: [{ waitFor: { text: "미디어 연결에 실패했습니다" } }, { wait: 200 }],
    }),

    // ---------- 시청 재생 중(가짜 RTCPeerConnection + 캔버스 무늬, D-077) ----------
    s("b-watch-playing", {
      note: "하린 방송 재생 중: 왼쪽 위 빨간 '나가기', 오른쪽 위는 시청자 수(눈 + 3)만, 통계·새 창 단추는 아이콘만. 배포 빌드라 통계 패널은 꺼진 채 시작",
      hash: watchHash(harin),
      mock: watchMock(watchPlaying),
      steps: [...watchPlayingReady, { waitFor: { role: "button", name: "통계" } }, { wait: 300 }],
    }),
    s("b-watch-playing-idle", {
      note: "b-watch-playing에서 마우스를 4초 움직이지 않음: 위·아래 막대가 사라지고 영상 위 커서도 숨김",
      hash: watchHash(harin),
      mock: watchMock(watchPlaying),
      steps: [...watchPlayingReady, { wait: 4000 }, { waitFor: { css: ".root.idle" } }],
    }),
    s("b-watch-playing-stats", {
      note: "통계를 켠 뒤 영상 위에서 4초 기다림: 막대는 사라지고 사용자가 켠 통계 패널은 남는다",
      hash: watchHash(harin),
      mock: watchMock(watchPlaying),
      steps: [
        { waitFor: { css: "video.shown" } },
        { click: { role: "button", name: "통계" } },
        { waitFor: { css: "section.stats" } },
        { hover: { css: "video" } },
        { wait: 4000 },
        { waitFor: { css: ".root.idle" } },
      ],
    }),
    s("b-watch-playing-popout", {
      note: "새 창으로 연 시청 창(해시 popout=1): 빨간 '창 닫기', 새 창 단추 없음",
      viewport: { width: 1280, height: 760 },
      hash: watchHash(harin) + "?popout=1",
      mock: watchMock(watchPlaying),
      steps: [...watchPlayingReady, { waitFor: { role: "button", name: "창 닫기" } }, { wait: 300 }],
    }),

    // ---------- 방송 ----------
    s("b-broadcast-setup", {
      note: "방송 시작: 'ELDEN RING™' 창을 고르고 로컬 랜 호스팅인 상태",
      viewport: { width: 1100, height: 860 },
      hash: "#/broadcast",
      steps: [
        { waitFor: { text: "ELDEN RING™" } },
        { click: { text: "ELDEN RING™" } },
        { click: { text: "로컬 랜", exact: true } },
        { wait: 300 },
      ],
    }),
    s("b-broadcast-setup-low", {
      note: "방송 시작: 비트레이트 1.5 Mbps를 쳐서 '부족' 구간과 뭉개짐 경고(시안 없음, D-074)",
      viewport: { width: 1100, height: 980 },
      hash: "#/broadcast",
      steps: [
        { waitFor: { text: "ELDEN RING™" } },
        { click: { text: "ELDEN RING™" } },
        { fill: { label: "비트레이트", exact: true }, value: "1.5" },
        { press: "Enter" },
        { waitFor: { text: "뭉개지고" } },
        { wait: 200 },
      ],
    }),
    s("b-broadcast-setup-high", {
      note: "방송 시작: 원본 해상도 + 144(60Hz 화면이라 안내) + 40 Mbps '충분', 로컬 랜 업로드 경고(시안 없음, D-074)",
      viewport: { width: 1100, height: 980 },
      hash: "#/broadcast",
      steps: [
        { waitFor: { text: "ELDEN RING™" } },
        { click: { text: "ELDEN RING™" } },
        { click: { text: "로컬 랜", exact: true } },
        { click: { text: "원본", exact: true } },
        { click: { text: "144", exact: true } },
        { waitFor: { text: "60fps까지만" } },
        { fill: { label: "비트레이트", exact: true }, value: "40" },
        { press: "Tab" },
        { waitFor: { text: "업로드가 40 Mbps" } },
        { wait: 200 },
      ],
    }),
    s("b-live", {
      note: "방송 중: 직접 호스팅, 포트 7884, UPnP 열림, Discord 빼는 중, 시청자 2명, 비밀번호 걸림",
      viewport: { width: 1100, height: 900 },
      hash: "#/broadcast",
      mock: { broadcast_status: fx.broadcastLive },
      steps: [{ waitFor: { text: "초대 코드" } }, { wait: 300 }],
    }),
    s("b-live-quality", {
      note: "방송 중 화질 변경: 비트레이트 30 Mbps를 치고 Enter → 바로 적용(적용됨), '충분' + 시청자 2명 업로드 경고(시안 없음, D-074)",
      viewport: { width: 1100, height: 1240 },
      hash: "#/broadcast",
      mock: {
        broadcast_status: fx.broadcastLive,
        update_broadcast: fn((args, _ctx, d) => ({ ...d, settings: args.settings }), fx.broadcastLive),
      },
      steps: [
        { waitFor: { text: "초대 코드" } },
        { fill: { label: "비트레이트", exact: true }, value: "30" },
        { press: "Enter" },
        { waitFor: { text: "적용됨" } },
        { waitFor: { text: "지금 2명이면 60 Mbps" } },
        { wait: 200 },
      ],
    }),
    s("b-live-notices", {
      note: "방송 중: 포트 7885 대체, UPnP 이중 NAT, Discord 실행 중 아님",
      viewport: { width: 1100, height: 1000 },
      hash: "#/broadcast",
      mock: { broadcast_status: fx.broadcastNotices },
      steps: [{ waitFor: { text: "초대 코드" } }, { wait: 300 }],
    }),
  ];
}
