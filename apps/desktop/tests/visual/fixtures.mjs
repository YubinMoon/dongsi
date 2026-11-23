// 화면 비교 하네스의 고정 데이터.
// 이름·주소·숫자는 예전 B 디자인 시안(지금은 지움)에 있던 값을 그대로 쓰고, 타입은 src/lib/types.ts와 맞춘다.
// 시각이 들어가는 값(방송 시작 시각)은 실행할 때의 지금 시각에서 계산한다.

/** @typedef {import("../../src/lib/types").ServerView} ServerView */

const VIDEO_CODECS = ["h264", "av1"];

/** 앞 8바이트(콜론 16진수)가 주어진 SHA-256 지문처럼 보이는 값. 나머지 24바이트는 결정적으로 채운다. */
function fingerprint(head8) {
  const bytes = head8.split(":").map((h) => parseInt(h, 16));
  let seed = bytes.reduce((a, b) => (a * 31 + b) >>> 0, 7);
  while (bytes.length < 32) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    bytes.push(seed >>> 24);
  }
  const hex = (b) => b.toString(16).toUpperCase().padStart(2, "0");
  return {
    short: bytes.slice(0, 8).map(hex).join(":"),
    full: bytes.map(hex).join(":"),
  };
}

/** base64url(JSON) 초대 코드. 시안의 b-add-server 코드와 같은 모양({a, f, b, n}). */
function inviteCode(obj) {
  const json = JSON.stringify(obj);
  return "dongsi1:" + Buffer.from(json, "utf8").toString("base64url");
}

/**
 * @param {number} [nowMs] 기준 시각(밀리초). 기본은 지금.
 */
export function buildFixtures(nowMs = Date.now()) {
  const now = Math.floor(nowMs / 1000);
  /** 방송 시작 시각: 시안에 적힌 "N분 전"이 되게 조금 더 지난 값으로 잡는다(경계 흔들림 방지). */
  const ago = (seconds) => now - seconds;

  // ---------- 설정 ----------
  const settings = { displayName: "지우", installId: "a1b2c3d4e5f6g7h8i9j0kA" };

  // ---------- 서버 목록(시안: 우리집 서버, 민수 PC, 회사 테일넷) ----------
  const fpHome = fingerprint("3A:9F:12:C4:7B:E0:55:D1");
  const fpMinsu = fingerprint("5C:21:E8:0D:94:AB:36:F7");
  const fpCorp = fingerprint("6D:48:A1:73:0E:C9:2B:84");
  const fpFriend = fingerprint("3A:9F:12:C4:7B:E0:55:D1"); // 시안의 초대 코드 지문과 같은 앞부분
  const fpActual = fingerprint("B7:0E:5D:1C:A2:68:F3:9E"); // 지문이 다를 때 실제로 받은 값
  const fpStudy = fingerprint("8E:13:C7:42:5A:D9:06:BB");
  const fpClub = fingerprint("47:A0:9C:E5:12:6F:D8:33");
  const fpClubActual = fingerprint("C2:5B:81:F4:3D:07:AE:69");

  /** @type {ServerView} */
  const home = {
    id: "Hq7mZ2kXp9WvT4nR8sLc1A",
    name: "우리집 서버",
    addrs: ["dongsi.example.net", "100.101.102.103"],
    fingerprint: fpHome,
    savedPasswords: 1,
  };
  /** @type {ServerView} */
  const minsu = {
    id: "Bv3dN8yQe1UzK6tJ0xGfMw",
    name: "민수 PC",
    addrs: ["192.168.0.24"],
    fingerprint: fpMinsu,
    savedPasswords: 0,
  };
  /** @type {ServerView} 첫 접속 전이라 지문이 없다 */
  const corp = {
    id: "Ym5rC0aHj7SpL2wD9qVuXg",
    name: "회사 테일넷",
    addrs: ["100.88.12.7:7900"],
    fingerprint: null,
    savedPasswords: 0,
  };
  /** b-home-toast에서 추가되는 서버 */
  /** @type {ServerView} */
  const friend = {
    id: "Tn4eF6hZb1KxR8vM3cWoYs",
    name: "친구의 서버",
    addrs: ["203.0.113.7", "192.168.0.40"],
    fingerprint: fpFriend,
    savedPasswords: 0,
  };

  /** b-tabs(탭 상태 섞임)에만 나오는 서버 둘: 사용자가 눌러 실패, 인증서 지문 다름 */
  /** @type {ServerView} */
  const study = {
    id: "Kd2wS9gTq6LbY1xN4mRfHe",
    name: "스터디 서버",
    addrs: ["study.example.org"],
    fingerprint: fpStudy,
    savedPasswords: 0,
  };
  /** @type {ServerView} */
  const club = {
    id: "Wz7cV3nJa0PrE5kU8hTyBs",
    name: "동아리 서버",
    addrs: ["club.example.org:7884"],
    fingerprint: fpClub,
    savedPasswords: 0,
  };

  // ---------- 우리집 서버의 방송 3개(b-server) ----------
  const harin = {
    broadcasterId: "Xc2pQ7vL9mNe4TzR1bKwYa",
    broadcasterName: "하린",
    codec: "h264",
    width: 1920,
    height: 1080,
    fps: 60,
    hasAudio: true,
    locked: false,
    viewers: 3,
    startedAt: ago(30 * 60 + 20),
    passwordSaved: false,
    mine: false,
  };
  const doyun = {
    broadcasterId: "Lw8sD3fG5hJ1kZ6xC9vBnQ",
    broadcasterName: "도윤",
    codec: "av1",
    width: 3840,
    height: 2160,
    fps: 30,
    hasAudio: true,
    locked: true,
    viewers: 6,
    startedAt: ago(90 * 60 + 20),
    passwordSaved: true,
    mine: false,
  };
  const seoyeon = {
    broadcasterId: "Pf1aE4uI7oY2tR5eW8qZmK",
    broadcasterName: "서연 · 엘든 링 보스전",
    codec: "h264",
    width: 2560,
    height: 1440,
    fps: 60,
    hasAudio: true,
    locked: true,
    viewers: 1,
    startedAt: ago(75),
    passwordSaved: false,
    mine: false,
  };

  /** connect_server가 돌려주는 값(서버 id 키). */
  const live = {
    [home.id]: {
      key: home.id,
      serverName: home.name,
      canPublish: true,
      hostOnly: false,
      fingerprint: fpHome,
      broadcasts: [harin, doyun, seoyeon],
    },
    [minsu.id]: {
      key: minsu.id,
      serverName: minsu.name,
      canPublish: true,
      hostOnly: false,
      fingerprint: fpMinsu,
      broadcasts: [],
    },
    [corp.id]: {
      key: corp.id,
      serverName: corp.name,
      canPublish: true,
      hostOnly: false,
      fingerprint: fpCorp,
      broadcasts: [],
    },
  };

  // ---------- 로컬 감지(같은 네트워크: 준호 LAN, 유나 Tailscale) ----------
  const localServers = [
    {
      key: "local-192.168.0.31:7884",
      name: "준호의 PC",
      addr: "192.168.0.31:7884",
      via: "lan",
      hostOnly: true,
      saved: false,
      broadcasts: [
        {
          broadcasterId: "Qm6nB2vC8xZ4lK1jH7gFdS",
          broadcasterName: "준호",
          codec: "h264",
          width: 1920,
          height: 1080,
          fps: 60,
          locked: false,
          passwordSaved: false,
        },
      ],
    },
    {
      key: "local-100.101.55.8:7884",
      name: "유나 노트북",
      addr: "100.101.55.8:7884",
      via: "tailscale",
      hostOnly: true,
      saved: false,
      broadcasts: [
        {
          broadcasterId: "Rt3yU9iO5pA1sD7fG2hJkL",
          broadcasterName: "유나",
          codec: "av1",
          width: 2560,
          height: 1440,
          fps: 60,
          locked: true,
          passwordSaved: false,
        },
      ],
    },
  ];

  // ---------- 서버 추가(초대 코드, b-add-server · b-home-toast) ----------
  // 시안 b-add-server의 입력란에 들어 있는 코드 그대로.
  const sampleInvite =
    "dongsi1:eyJhIjpbIjIwMy4wLjExMy43IiwiMTkyLjE2OC4wLjQwIl0sImYiOiJBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWjAxMjM0NTY3ODlhYmMiLCJiIjoiYTFiMmMzZDRlNWY2ZzdoOGk5ajBrQSIsIm4iOiLsuZzqtazsnZgg7ISc67KEIn0";
  const parsedInvite = {
    kind: "invite",
    name: "친구의 서버",
    addrs: friend.addrs,
    fingerprint: fpFriend,
    broadcast: true,
    hasPassword: false,
    existing: null,
  };
  const addResult = { outcome: "added", server: friend };

  // ---------- 방송 시작(b-broadcast-setup) ----------
  const sources = [
    { id: "monitor:65537", kind: "monitor", name: "화면 1 (DELL U2723QE)", width: 3840, height: 2160, primary: true, processName: null, discord: false, follow: null, refreshRate: 60 },
    { id: "monitor:65541", kind: "monitor", name: "화면 2 (LG 24MP)", width: 1920, height: 1080, primary: false, processName: null, discord: false, follow: null, refreshRate: 75 },
    { id: "window:526404", kind: "window", name: "ELDEN RING™", width: 2560, height: 1440, primary: false, processName: "eldenring.exe", discord: false, follow: null, refreshRate: 60 },
    { id: "window:788120", kind: "window", name: "YouTube - Chrome", width: 1920, height: 1040, primary: false, processName: "chrome.exe", discord: false, follow: null, refreshRate: 60 },
    { id: "window:330992", kind: "window", name: "dongsi — Visual Studio Code", width: 1720, height: 1000, primary: false, processName: "Code.exe", discord: false, follow: null, refreshRate: 75 },
  ];
  /** 미리 보기: 단색 바탕 + 띠 하나(SVG). 실제는 JPEG data URL. 화면 2는 그림 없음, VS Code 창은 최소화. */
  const shot = (w, h, bg, band) =>
    "data:image/svg+xml;base64," +
    btoa(
      `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="100%" height="100%" fill="${bg}"/><rect y="${h * 0.62}" width="100%" height="${h * 0.16}" fill="${band}"/></svg>`,
    );
  const thumbnails = [
    { id: "monitor:65537", image: shot(480, 270, "#1f3b57", "#d9e2ec"), minimized: false },
    { id: "monitor:65541", image: null, minimized: false },
    { id: "window:526404", image: shot(480, 270, "#3a2f1d", "#c9a45c"), minimized: false },
    { id: "window:788120", image: shot(480, 260, "#202124", "#e8eaed"), minimized: false },
    { id: "window:330992", image: null, minimized: true },
  ];
  /**
   * 방송 목록 썸네일(D-076, broadcast_thumbnail): "<서버 key>/<방송자 ID>" → 그림. 실제는 JPEG data URL.
   * 하린(우리집 서버)만 그림이 있고, 준호(같은 네트워크)는 아직 그림이 없다(자리 표시). 비밀번호 방송(도윤·서연·유나)은
   * 서버가 그림을 받지 않아 화면이 묻지도 않는다(자물쇠 그림).
   */
  const scene = (w, h) =>
    "data:image/svg+xml;base64," +
    btoa(
      `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2a3f63"/><stop offset="1" stop-color="#c77d4a"/></linearGradient></defs><rect width="100%" height="100%" fill="url(#s)"/><circle cx="${w * 0.72}" cy="${h * 0.42}" r="${h * 0.12}" fill="#f4d28c"/><path d="M0 ${h * 0.78} L${w * 0.25} ${h * 0.55} L${w * 0.45} ${h * 0.72} L${w * 0.7} ${h * 0.5} L${w} ${h * 0.74} L${w} ${h} L0 ${h} Z" fill="#1d2433"/></svg>`,
    );
  const broadcastThumbnails = {
    [home.id + "/" + harin.broadcasterId]: { id: 1, image: scene(480, 270) },
  };
  /** 시안의 "h264_nvenc · 하드웨어"가 보이게 H.264는 하드웨어, 나머지는 있는 것으로. */
  const encoders = [
    { codec: "h264", name: "h264_nvenc", hardware: true },
    { codec: "av1", name: "av1_nvenc", hardware: true },
  ];
  /** WebView2가 재생할 수 있는 코덱(H.264·AV1 둘 다). mock-ipc.js가 RTCRtpReceiver.getCapabilities에 넣는다. */
  const receivableCodecs = ["video/H264", "video/AV1"];

  // ---------- 방송 중(b-live, b-live-notices) ----------
  /** `limited`: 가장 느린 시청자에 맞춰 비디오 목표를 낮춘 상태(D-081, b-live-notices). simulcast(D-102, 상 + 하 FHD 3 Mbps)는 b-live만. */
  const layerStats = (layer, kbps) => ({
    layer,
    encoder: "h264_nvenc",
    width: 1920,
    height: 1080,
    targetKbps: kbps,
    encodeFps: 60,
    avgEncodeMs: 1.8,
    bitrateKbps: kbps * 0.98,
    droppedFrames: 0,
    frames: 45200,
    keyframes: 3,
    softwarePaused: false,
    lastError: null,
  });
  const stats = (discordExcluded, limited = false) => ({
    simulcast: !limited,
    layers: limited ? [] : [layerStats("low", 3000)],
    publish: {
      connected: true,
      closed: null,
      videoKbps: limited ? 2480 : 9950,
      audioKbps: 128,
      rttMs: 4,
      loss: 0,
      nacks: 0,
      keyframeRequests: 1,
      droppedFrames: 0,
      targetKbps: limited ? 2500 : 7000,
      maxKbps: 7000,
      estimateKbps: limited ? 2950 : 18400,
      rembKbps: limited ? 2950 : null,
      limited,
      layers: limited ? null : [7000, 3000],
    },
    video: {
      encoder: "h264_nvenc",
      hardware: true,
      codec: "h264",
      width: 1920,
      height: 1080,
      sourceWidth: 2560,
      sourceHeight: 1440,
      captureFps: 60,
      encodeFps: 60,
      avgEncodeMs: 2.1,
      bitrateKbps: 6950,
      droppedFrames: 0,
      avgConvertMs: 0.4,
      avgLatencyMs: 18,
      gpuConvert: true,
      frames: 45240,
      keyframes: 3,
    },
    audioStats: {
      mode: "systemExceptDiscord",
      discordExcluded,
      levelDbfs: -18,
      packets: 37700,
      encoderDelaySamples: 312,
      silenceFilledMs: 0,
      droppedMs: 0,
      lastError: null,
    },
  });
  const liveBase = {
    hosting: "direct",
    key: "host",
    broadcasterId: settings.installId,
    serverName: "지우",
    sourceId: "window:526404",
    sourceName: "ELDEN RING™",
    sourceWaiting: false,
    audio: "systemExceptDiscord",
    settings: { codec: "h264", width: 1920, height: 1080, fps: 60, bitrateKbps: 7000 },
    width: 1920,
    height: 1080,
    fps: 60,
    locked: true,
    viewers: 2,
    // 시안: 위쪽 바 "12:34", 타일 "12분"
    startedAt: ago(12 * 60 + 34),
    videoError: null,
  };
  const inviteLive = (port, host) =>
    inviteCode({ a: [host + ":" + port], f: fpHome.full.replaceAll(":", "").slice(0, 43), b: settings.installId, n: "지우", p: "demo-pass" });
  const inviteLiveNoPassword = (port, host) =>
    inviteCode({ a: [host + ":" + port], f: fpHome.full.replaceAll(":", "").slice(0, 43), b: settings.installId, n: "지우" });

  /** b-live: 직접 호스팅, 포트 7884, UPnP 열림, Discord 소리 빼는 중 */
  const broadcastLive = {
    ...liveBase,
    invite: inviteLive(7884, "203.0.113.7"),
    inviteWithoutPassword: inviteLiveNoPassword(7884, "203.0.113.7"),
    port: 7884,
    requestedPort: 7884,
    portFallback: false,
    upnp: { external: "203.0.113.7:7884", doubleNat: false, error: null },
    ...stats(true),
  };
  /** b-live-notices: 포트 7885(7884 사용 중), UPnP 외부 IP가 사설 대역(이중 NAT), Discord 실행 중 아님, 가장 느린 시청자에 맞춰 화질을 낮춤 */
  const broadcastNotices = {
    ...liveBase,
    invite: inviteLive(7885, "100.64.12.7"),
    inviteWithoutPassword: inviteLiveNoPassword(7885, "100.64.12.7"),
    port: 7885,
    requestedPort: 7884,
    portFallback: true,
    upnp: { external: "100.64.12.7:7885", doubleNat: true, error: null },
    ...stats(false, true),
  };

  return {
    now,
    settings,
    servers: { home, minsu, corp, friend, study, club },
    live,
    broadcasts: { harin, doyun, seoyeon },
    localServers,
    sampleInvite,
    parsedInvite,
    addResult,
    fingerprints: { home: fpHome, actual: fpActual, club: fpClub, clubActual: fpClubActual },
    sources,
    thumbnails,
    broadcastThumbnails,
    encoders,
    receivableCodecs,
    broadcastLive,
    broadcastNotices,
    codecs: VIDEO_CODECS,
  };
}
