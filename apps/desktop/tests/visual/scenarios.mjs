// 시안 하나당 시나리오 하나. 이름은 .docs/design/canvas/b-*.dc.html 파일 이름과 같다.
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
//   { card: id }               서버 카드(data-server-id)
//   { cardButton: id, name }   서버 카드 안 본문 단추·링크(⋯ 메뉴 단추 제외, 접근 이름에 서버 이름)
//   { cardMenu: name }         서버 카드 ⋯ 단추(aria-label "'이름' 서버 메뉴")
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

/** 거의 모든 화면이 부르는 명령의 기본 표. 시나리오가 덮어쓴다. */
function baseTable(fx, { connected }) {
  const { home, minsu, corp } = fx.servers;
  return {
    get_settings: fx.settings,
    list_servers: [home, minsu, corp],
    scan_local: fx.localServers,
    connected_servers: connected.map((s) => s.id),
    disconnect_server: null,
    connect_server: fn((args, _ctx, live) => {
      if (live[args.key]) return live[args.key];
      throw { message: "서버를 찾을 수 없습니다" };
    }, fx.live),
    broadcast_status: null,
    list_capture_sources: fx.sources,
    available_encoders: fx.encoders,
    quality_presets: fx.presets,
    stop_watch: null,
    request_keyframe: null,
  };
}

export function createScenarios(fx = buildFixtures()) {
  const { home, minsu, corp, friend } = fx.servers;
  const { harin, doyun } = fx.broadcasts;
  const two = [home, minsu]; // 탭이 "우리집 서버", "민수 PC" 둘인 시안
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

  /** 홈 화면이 떴는지: 우리집 서버 카드와 같은 네트워크 타일 */
  const homeReady = [{ waitFor: { card: home.id } }, { waitFor: { text: "준호" } }];
  const connectCorp = [{ click: { cardButton: corp.id, name: corp.name } }];

  const fingerprintError = {
    message:
      "서버 인증서 지문이 저장된 것과 다릅니다(저장 3A:9F:12:C4…, 받은 값 B7:0E:5D:1C…). 서버를 다시 설치했다면 목록에서 삭제하고 새 초대 코드로 추가해 주세요.",
    code: "fingerprintMismatch",
    detail: {
      stored: fx.fingerprints.home,
      actual: fx.fingerprints.actual,
    },
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

  return [
    // ---------- 홈 ----------
    s("b-home", {
      note: "서버 3개, 탭(우리집 서버·민수 PC), 같은 네트워크 방송 2개",
      steps: homeReady,
    }),
    s("b-first-run", {
      note: "서버 0개, 같은 네트워크 방송 없음(찾는 중…)",
      connected: [],
      mock: { list_servers: [], scan_local: [], connected_servers: [] },
      steps: [{ wait: 600 }],
    }),
    s("b-home-connecting", {
      note: "회사 테일넷 카드를 눌러 connect_server가 끝나지 않는 상태",
      mock: { connect_server: pending() },
      steps: [...homeReady, ...connectCorp, { wait: 400 }],
    }),
    s("b-home-connect-failed", {
      note: "회사 테일넷 접속 실패(시간 초과)",
      mock: { connect_server: reject({ message: "접속하지 못했습니다(시간 초과)" }) },
      steps: [...homeReady, ...connectCorp, { waitFor: { text: "시간 초과" } }],
    }),
    s("b-home-fingerprint", {
      note: "우리집 서버 카드: 인증서 지문 불일치(탭은 민수 PC만)",
      connected: [minsu],
      mock: { connect_server: reject(fingerprintError) },
      steps: [
        ...homeReady,
        { click: { cardButton: home.id, name: home.name } },
        { waitFor: { text: "지문이 다릅니다" } },
      ],
    }),
    s("b-server-menu", {
      note: "회사 테일넷 카드의 ⋯ 메뉴가 열린 모습",
      steps: [
        ...homeReady,
        { click: { cardMenu: corp.name } },
        { waitFor: { menuitem: "이름 변경" } },
      ],
    }),
    s("b-rename", {
      note: "회사 테일넷 이름 변경 대화 상자(입력란에 현재 이름)",
      steps: [
        ...homeReady,
        { click: { cardMenu: corp.name } },
        { click: { menuitem: "이름 변경" } },
        { waitFor: { role: "dialog" } },
        { wait: 200 },
      ],
    }),
    s("b-remove", {
      note: "회사 테일넷 삭제 확인 대화 상자",
      steps: [
        ...homeReady,
        { click: { cardMenu: corp.name } },
        { click: { menuitem: "삭제" } },
        { waitFor: { role: "dialog" } },
        { wait: 200 },
      ],
    }),
    s("b-home-toast", {
      note: "초대 코드로 '친구의 서버'를 추가한 직후(알림이 떠 있음, 목록 4개)",
      state: { servers: [home, minsu, corp] },
      mock: {
        list_servers: fn((_a, ctx) => ctx.state.servers),
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
      note: "우리집 서버 화면을 띄운 뒤 server-closed 이벤트로 연결이 끊긴 상태",
      hash: "#/server/" + home.id,
      steps: [
        { waitFor: { tile: "하린" } },
        { emit: ["server-closed", { key: home.id, reason: "서버가 닫혔습니다." }] },
        { waitFor: { text: "연결이 끊어졌습니다" } },
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

    // ---------- 시청(재생 화면 b-watch, b-watch-popout은 실제 WebRTC가 필요해 뺀다) ----------
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
    s("b-live", {
      note: "방송 중: 직접 호스팅, 포트 7884, UPnP 열림, Discord 빼는 중, 시청자 2명, 비밀번호 걸림",
      viewport: { width: 1100, height: 900 },
      hash: "#/broadcast",
      mock: { broadcast_status: fx.broadcastLive },
      steps: [{ waitFor: { text: "초대 코드" } }, { wait: 300 }],
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
