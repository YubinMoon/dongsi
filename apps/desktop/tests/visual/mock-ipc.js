// @ts-nocheck
// 브라우저에 page.addInitScript로 넣는 독립 스크립트: Tauri IPC(window.__TAURI_INTERNALS__)를 가짜로 바꾼다.
//
// @tauri-apps/api 2.12의 core.js(invoke, transformCallback), event.js(listen/unlisten),
// mocks.js(mockIPC의 shouldMockEvents), window.js(getCurrentWindow가 읽는 metadata),
// app.js(getVersion = plugin:app|version)가 기대하는 모양을 그대로 흉내 낸다.
//
// 이 스크립트 앞에서 러너가 window.__MOCK_CONFIG__를 넣어 준다:
//   { table, state, windowLabel }
// table: 명령 이름 → 항목. 항목은 다음 중 하나다.
//   - 태그 없는 값            → 그 값으로 성공
//   - { __mock: "value", value, delay? }   → value로 성공(delay ms 뒤)
//   - { __mock: "error", value, delay? }   → value로 거절(Rust CmdError 모양 { message, code?, detail? })
//   - { __mock: "pending" }                → 영원히 대기
//   - { __mock: "fn", source, data? }      → source는 (args, ctx, data) => 값 형태의 함수 소스.
//                                            값을 돌려주면 성공, throw하면 그 값으로 거절. ctx는 아래.
// ctx: { state, emit(event, payload), calls, cmd }
(() => {
  const cfg = window.__MOCK_CONFIG__ || {};
  const table = cfg.table || {};
  const state = cfg.state || {};

  /** 시나리오 작성 때 빠진 명령이 드러나게 모아 둔다. */
  const log = { calls: [], unhandled: [] };
  window.__mock = { calls: log.calls, unhandled: log.unhandled, state, listeners: null };

  window.isTauri = true;
  window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = window.__TAURI_EVENT_PLUGIN_INTERNALS__ || {};

  // ---------- 콜백(core.js transformCallback / mocks.js) ----------
  const callbacks = new Map();
  function registerCallback(callback, once = false) {
    const id = window.crypto.getRandomValues(new Uint32Array(1))[0];
    callbacks.set(id, (data) => {
      if (once) callbacks.delete(id);
      return callback && callback(data);
    });
    return id;
  }
  function unregisterCallback(id) {
    callbacks.delete(id);
  }
  function runCallback(id, data) {
    const cb = callbacks.get(id);
    if (cb) cb(data);
    else console.warn("[TAURI] Couldn't find callback id " + id + ".");
  }

  // ---------- 이벤트 플러그인(mocks.js의 shouldMockEvents) ----------
  const listeners = new Map(); // event -> handler id 배열
  window.__mock.listeners = listeners;
  function handleEventPlugin(cmd, args) {
    switch (cmd) {
      case "plugin:event|listen": {
        if (!listeners.has(args.event)) listeners.set(args.event, []);
        listeners.get(args.event).push(args.handler);
        return args.handler; // eventId
      }
      case "plugin:event|unlisten": {
        const list = listeners.get(args.event);
        if (list) {
          const i = list.indexOf(args.eventId);
          if (i !== -1) list.splice(i, 1);
        }
        return null;
      }
      case "plugin:event|emit":
      case "plugin:event|emit_to":
        emit(args.event, args.payload);
        return null;
    }
    return null;
  }

  /** Rust가 이벤트를 쏘는 것처럼 리스너를 부른다. 핸들러는 { event, id, payload }를 받는다. */
  function emit(event, payload) {
    for (const handler of [...(listeners.get(event) || [])]) {
      runCallback(handler, { event, id: handler, payload });
    }
  }
  window.__mockEmit = emit;

  // ---------- 명령 처리 ----------
  const ctx = { state, emit, calls: log.calls };
  const fnCache = new Map();
  function compile(source) {
    if (!fnCache.has(source)) fnCache.set(source, (0, eval)("(" + source + ")"));
    return fnCache.get(source);
  }
  const clone = (v) => (v === undefined ? v : JSON.parse(JSON.stringify(v)));
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));

  // 시나리오와 상관없이 앱이 부르는 것들.
  const builtin = {
    "plugin:app|version": "0.1.0",
    "plugin:app|name": "dongsi",
    dev_log: null,
    watch_log: null,
    app_ready: null,
  };

  async function invoke(cmd, args, _options) {
    log.calls.push({ cmd, args: clone(args) });
    if (cmd.startsWith("plugin:event|")) return handleEventPlugin(cmd, args);

    const entry = Object.prototype.hasOwnProperty.call(table, cmd)
      ? table[cmd]
      : Object.prototype.hasOwnProperty.call(builtin, cmd)
        ? builtin[cmd]
        : undefined;
    if (entry === undefined) {
      log.unhandled.push(cmd);
      console.warn("[mock] 처리하지 않은 명령: " + cmd + " " + JSON.stringify(args));
      throw { message: "[mock] 처리하지 않은 명령: " + cmd, code: "mockUnhandled" };
    }

    const tagged = entry !== null && typeof entry === "object" && typeof entry.__mock === "string";
    if (!tagged) return clone(entry);
    if (entry.delay) await wait(entry.delay);
    switch (entry.__mock) {
      case "value":
        return clone(entry.value);
      case "error":
        throw clone(entry.value);
      case "pending":
        return new Promise(() => {});
      case "fn": {
        const fn = compile(entry.source);
        return clone(await fn(clone(args) || {}, { ...ctx, cmd }, entry.data));
      }
      default:
        throw { message: "[mock] 알 수 없는 항목 종류: " + entry.__mock };
    }
  }

  window.__TAURI_INTERNALS__.invoke = invoke;
  window.__TAURI_INTERNALS__.transformCallback = registerCallback;
  window.__TAURI_INTERNALS__.unregisterCallback = unregisterCallback;
  window.__TAURI_INTERNALS__.runCallback = runCallback;
  window.__TAURI_INTERNALS__.callbacks = callbacks;
  window.__TAURI_INTERNALS__.convertFileSrc = (filePath, protocol = "asset") =>
    "http://" + protocol + ".localhost/" + encodeURIComponent(filePath);
  // window.js getCurrentWindow()가 읽는다.
  const label = cfg.windowLabel || "main";
  window.__TAURI_INTERNALS__.metadata = {
    currentWindow: { label },
    currentWebview: { windowLabel: label, label },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener = (_event, id) => unregisterCallback(id);
})();
