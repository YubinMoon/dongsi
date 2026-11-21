// 페이지 안에서 도는 스크립트(문자열). 안전장치와 RTCPeerConnection 수집, 통계 읽기.

/**
 * 안전장치 + PeerConnection 수집. addInitScript로 모든 문서에 넣고, 이미 떠 있는 문서에도 evaluate로 넣는다.
 * 여러 번 실행해도 안전하다.
 *
 * Tauri의 window.__TAURI_INTERNALS__.invoke는 쓰기·재정의가 불가능해서 제자리에서 감쌀 수 없다.
 * 그래서 개발 모드 vite가 내려 주는 @tauri-apps/api 모듈의 invoke()를 앱 쪽에서 window.__t4invoke를 거치게
 * 패치하고(app.mjs의 context.route, INVOKE_PATCH), 이 스크립트가 __t4invoke를 정의한다.
 *
 *  - list_capture_sources: 테스트 패턴(kind === "test")만 남긴다. 사용자 창 제목이 화면·스크린샷에 나오지 않게.
 *  - capture_source_thumbnails: Rust까지 보내지 않고 빈 목록을 돌려준다. 사용자 화면이 스크린샷에 나오지 않게.
 *  - start_broadcast: source가 "test"가 아니거나 audio가 "testTone"이 아니면 호출하지 않고 오류를 던진다.
 *  - RTCPeerConnection: 만들어진 객체를 window.__t4pcs에 모은다.
 *  - 첫 프레임 분해용 시각 기록(동작은 바꾸지 않는다): window.__t7pc[]에 PC마다 { ctorStart, created, offer, local, remote, connected }(Date.now).
 *    createOffer·setLocalDescription·setRemoteDescription이 끝난 시각, connectionstatechange가 처음 "connected"가 된 시각.
 */
export const GUARD_SCRIPT = `(() => {
  window.__t4guard = window.__t4guard || { installedAt: Date.now(), blocked: [], hidden: 0, calls: 0 };
  const guard = window.__t4guard;
  if (!window.__t4invoke) {
    window.__t4invoke = async (cmd, args, opts) => {
      guard.calls++;
      if (cmd === "capture_source_thumbnails") return [];
      if (cmd === "start_broadcast") {
        const o = (args && args.options) || {};
        if (o.source !== "test" || o.audio !== "testTone") {
          const msg = "[t4 안전장치] 테스트 패턴 + 테스트 톤이 아닌 방송은 시작하지 않습니다 (source=" + o.source + ", audio=" + o.audio + ")";
          guard.blocked.push(msg);
          throw { message: msg };
        }
      }
      const result = await window.__TAURI_INTERNALS__.invoke(cmd, args, opts);
      if (cmd === "list_capture_sources" && Array.isArray(result)) {
        const kept = result.filter((s) => s && s.kind === "test");
        guard.hidden += result.length - kept.length;
        return kept;
      }
      return result;
    };
  }
  const Orig = window.RTCPeerConnection;
  if (Orig && !Orig.__t4) {
    class T4PeerConnection extends Orig {
      constructor(...a) {
        const ctorStart = Date.now();
        super(...a);
        (window.__t4pcs = window.__t4pcs || []).push(this);
        try {
          const rec = { ctorStart, created: Date.now(), offer: null, local: null, remote: null, connected: null };
          (window.__t7pc = window.__t7pc || []).push(rec);
          const stamp = (name, key) => {
            const orig = Orig.prototype[name];
            if (typeof orig !== "function") return;
            this[name] = function (...args) {
              const p = orig.apply(this, args);
              if (p && typeof p.then === "function") p.then(() => { if (rec[key] === null) rec[key] = Date.now(); }, () => {});
              return p;
            };
          };
          stamp("createOffer", "offer");
          stamp("setLocalDescription", "local");
          stamp("setRemoteDescription", "remote");
          this.addEventListener("connectionstatechange", () => {
            if (this.connectionState === "connected" && rec.connected === null) rec.connected = Date.now();
          });
        } catch (e) {
          // 시각 기록은 부가 기능이다. 실패해도 안전장치·PC 수집에는 영향이 없어야 한다.
        }
      }
    }
    T4PeerConnection.__t4 = true;
    window.RTCPeerConnection = T4PeerConnection;
  }
})();`;

/** 안전장치가 걸려 있는지 확인(걸려 있지 않으면 false). */
export const GUARD_CHECK = `(() => !!(window.__t4guard && window.__t4invoke && window.RTCPeerConnection && window.RTCPeerConnection.__t4))()`;

/**
 * @tauri-apps/api의 invoke()를 패치한다: __t4invoke가 있으면 그것을 거치고, 없는데 위험한 명령이면 던진다(닫힌 쪽으로 실패).
 * 정규식은 tauri api의 core.js에 있는 한 줄에 맞춘다. 안 맞으면(0번 치환) 하네스가 자체 시험에서 알아채고 멈춘다.
 */
export const INVOKE_RE = /return\s+window\.__TAURI_INTERNALS__\.invoke\(\s*cmd\s*,\s*args\s*,\s*options\s*\)\s*;?/;
export const INVOKE_PATCH =
  'if (typeof window.__t4invoke !== "function" && (cmd === "start_broadcast" || cmd === "list_capture_sources" || cmd === "capture_source_thumbnails")) throw { message: "[t4 안전장치] 설치되지 않았습니다" };' +
  ' return (typeof window.__t4invoke === "function" ? window.__t4invoke(cmd, args, options) : window.__TAURI_INTERNALS__.invoke(cmd, args, options));';

/**
 * 자체 시험(페이지 안): 앱이 쓰는 api core 모듈의 invoke()가 안전장치를 거치는지 확인한다.
 *  - start_broadcast를 화면 소스·실제 소리로 부르면 던져야 한다(안전장치가 막아서 Rust까지 가지 않음).
 *  - list_capture_sources에 테스트 패턴이 아닌 것이 없어야 한다.
 * 반환: { ok, reason, hidden }
 */
export const SELFTEST_FN = `async () => {
  const urls = performance.getEntriesByType("resource").map((e) => e.name).filter((n) => /@tauri-apps_api_core\\.js/.test(n));
  if (!urls.length) return { ok: false, reason: "앱이 불러온 @tauri-apps/api core 모듈을 찾지 못했습니다" };
  const m = await import(urls[urls.length - 1]);
  let blocked = false;
  try {
    await m.invoke("start_broadcast", { options: { hosting: "lan", serverId: null, source: "t4-selftest-not-a-source", audio: "system", password: null, settings: {} } });
  } catch (e) {
    blocked = !!(e && typeof e.message === "string" && e.message.startsWith("[t4 안전장치]"));
  }
  if (!blocked) return { ok: false, reason: "start_broadcast가 안전장치에 막히지 않았습니다" };
  const list = await m.invoke("list_capture_sources");
  const bad = list.filter((s) => s.kind !== "test");
  if (bad.length) return { ok: false, reason: "list_capture_sources에 테스트 패턴이 아닌 소스가 " + bad.length + "개 남았습니다" };
  return { ok: true, sources: list.length, hidden: window.__t4guard ? window.__t4guard.hidden : null };
}`;

/**
 * 시청 PeerConnection의 통계를 읽는다(닫히지 않은 마지막 PC). 반환은 JSON으로 보낼 수 있는 값.
 * getStats는 호출한 쪽이 1초에 한 번 이하로 부른다.
 */
export const STATS_FN = `async () => {
  const pcs = (window.__t4pcs || []).filter((pc) => pc.connectionState !== "closed");
  const pc = pcs[pcs.length - 1];
  if (!pc) return { noPc: true, pcCount: (window.__t4pcs || []).length };
  const report = await pc.getStats();
  const nowMs = Date.now();
  const byId = {};
  let video = null, audio = null, pair = null;
  report.forEach((s) => {
    byId[s.id] = s;
    if (s.type === "inbound-rtp" && s.kind === "video") video = s;
    if (s.type === "inbound-rtp" && s.kind === "audio") audio = s;
    if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") pair = s;
  });
  const pick = (o, keys) => { if (!o) return null; const r = {}; for (const k of keys) if (o[k] !== undefined) r[k] = o[k]; return r; };
  return {
    nowMs,
    pcCount: (window.__t4pcs || []).length,
    connectionState: pc.connectionState,
    video: pick(video, ["timestamp", "framesDecoded", "framesReceived", "framesDropped", "frameWidth", "frameHeight", "framesPerSecond",
      "jitterBufferDelay", "jitterBufferEmittedCount", "estimatedPlayoutTimestamp", "packetsReceived", "packetsLost", "bytesReceived",
      "freezeCount", "keyFramesDecoded", "decoderImplementation", "codecId"]),
    audio: pick(audio, ["timestamp", "packetsReceived", "packetsLost", "totalSamplesReceived", "audioLevel", "totalAudioEnergy", "totalSamplesDuration",
      "jitterBufferDelay", "jitterBufferEmittedCount", "estimatedPlayoutTimestamp", "bytesReceived", "codecId"]),
    videoMime: video && video.codecId && byId[video.codecId] ? byId[video.codecId].mimeType : null,
    audioMime: audio && audio.codecId && byId[audio.codecId] ? byId[audio.codecId].mimeType : null,
    rttMs: pair && typeof pair.currentRoundTripTime === "number" ? pair.currentRoundTripTime * 1000 : null,
  };
}`;
