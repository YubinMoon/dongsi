<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { page } from "$app/state";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, errorCode, errorMessage } from "$lib/api";
  import { hueFor } from "$lib/hue";
  import { conn, connect, dismissCard } from "$lib/stores/connections.svelte";
  import { serverById } from "$lib/stores/servers.svelte";
  import { toast } from "$lib/toast.svelte";
  import type { BroadcastEndedEvent, BroadcastView, KeyEvent, NetworkPath, ServerClosedEvent } from "$lib/types";
  import {
    backHref as backHrefFor,
    cardFailureMessage,
    checkFrozen,
    classifyStartError,
    devLogLine,
    effectiveMuted,
    FREEZE_INITIAL,
    isPopoutHash,
    isTypingTarget,
    type LipSyncView,
    passwordSubtitle,
    shouldResume,
    showInfoBar,
    showLive,
    statRows,
    statusFromConnectionState,
    toggleMute,
    type FreezeState,
    type WatchStatus,
  } from "$lib/watch";
  import { LipSync, type LipSyncFloors, type LipSyncReason, type LipSyncState } from "$lib/lipsync";
  import { audioRedFromCodecs, audioRedFromSdp, collectStats, type WatchStats } from "$lib/watchStats";
  import { audioStall, bufferOptionsFor, PlayoutBuffer, videoStallMs, type FrameMark } from "$lib/playoutBuffer";
  import { WatchLogger } from "$lib/watchLog";
  import { isOverlayKey, OverlayIdle } from "$lib/overlayIdle";
  import PasswordForm from "$lib/components/watch/PasswordForm.svelte";
  import StageBackdrop from "$lib/components/watch/StageBackdrop.svelte";
  import StatusScreen from "$lib/components/watch/StatusScreen.svelte";
  import WatchControls from "$lib/components/watch/WatchControls.svelte";
  import WatchStatsPanel from "$lib/components/watch/WatchStatsPanel.svelte";
  import WatchTopBar from "$lib/components/watch/WatchTopBar.svelte";
  import "$lib/components/watch/watch.css";

  const key = $derived(page.params.key ?? "");
  const broadcasterId = $derived(page.params.broadcaster ?? "");
  const popout = typeof location !== "undefined" && isPopoutHash(location.hash);
  const backHref = $derived(backHrefFor(key));
  const hue = $derived(hueFor(broadcasterId));

  let status = $state<WatchStatus>("connecting");
  /** 오류 화면의 문장 */
  let message = $state("");
  /** 비밀번호 폼 입력란 아래 오류 줄 */
  let passwordError = $state<string | null>(null);
  /** 비밀번호를 보내는 중(폼에 머문 채 단추만 바쁨 표시) */
  let passwordBusy = $state(false);
  let info = $state<BroadcastView | null>(null);
  let serverName = $state("");
  let stats = $state<WatchStats | null>(null);
  /** 통계 패널: 개발 빌드도 배포 빌드도 꺼진 채 시작한다(통계 단추로 켠다). */
  let showStats = $state(false);
  let volume = $state(1);
  let muted = $state(false);
  let needsUnmute = $state(false);

  let video: HTMLVideoElement | undefined = $state();
  let root: HTMLDivElement | undefined = $state();

  let peer: RTCPeerConnection | null = null;
  let session: number | null = null;
  let statsTimer: ReturnType<typeof setTimeout> | null = null;
  let unlisten: UnlistenFn[] = [];
  let destroyed = false;
  let freeze: FreezeState = FREEZE_INITIAL;
  let prevStats: Map<string, RTCStats> | null = null;
  const isDev = import.meta.env.DEV;
  /** 개발용: 트랙별 MediaStream ID(같아야 브라우저가 A/V를 맞춘다) */
  let streamIds = $state<string[]>([]);
  let lastDevLog = 0;

  // 시청 로그(모든 빌드, D-088): 일이 있을 때와 10초 요약을 앱 로그에 남긴다(src/lib/watchLog.ts).
  const logger = new WatchLogger();
  /** 서버로 가는 경로(Wi-Fi 감지). 시청을 시작할 때 답에서 받고 10초마다 다시 읽는다. 모르면 null */
  let network = $state<NetworkPath | null>(null);
  let lastNetworkAt = Number.NEGATIVE_INFINITY;
  const NETWORK_REFRESH_MS = 10_000;
  /** 개발 빌드: localStorage "dongsi.buffer"가 "off"면 시청 버퍼를 끈다 */
  let bufferOff = false;

  // 립싱크 보정(src/lib/lipsync.ts, D-060)은 기본으로 끈다(D-093): 브라우저 자체 A/V 동기화가 약 20초에 걸쳐 멈춤 없이 맞춘다.
  const lipsync = new LipSync();
  /** 보정기를 쓰는가. 개발 빌드에서 localStorage "dongsi.lipsync"가 "on"이면 켠다(전·후 비교용, D-093). */
  let lipsyncOn = false;
  /** 직전 통계의 코덱·해상도, 비디오 디코드 프레임 수(바뀌면 streamChanged) */
  let lastStreamSig: string | null = null;
  let lastDecoded = 0;
  /** 직전 통계의 오디오 은닉 샘플 누적(개발 빌드 기록의 concealedDelta용) */
  let lastConcealed: number | null = null;
  /** 개발 빌드: 통계 패널의 "립싱크 보정" 줄 */
  let lipsyncView = $state<LipSyncView | null>(null);

  // 시청 버퍼(D-072): 두 수신기에 같이 거는 최소 재생 지연 B. 끊기면 올리고 조용하면 내린다(src/lib/playoutBuffer.ts).
  // 수신기에 거는 값은 B + 립싱크 보정(그 트랙 쪽)이다.
  let buffer = new PlayoutBuffer();
  /** 지금 수신기에 건 B(통계 패널 "버퍼 목표" 줄) */
  let bufferMs = $state(buffer.ms);
  /** 직전에 화면에 나온 프레임(rVFC). 해상도가 바뀌거나 창이 숨겨지면 새로 잰다 */
  let lastFrame: (FrameMark & { width: number; height: number }) | null = null;
  /** 직전 통계의 오디오 은닉 샘플·수신·손실 누적(오디오 끊김 감지) */
  let prevAudio: { concealed: number; packets: number; lost: number } | null = null;
  /** 오디오 RED를 협상했는가(D-087). 통계 패널·시청 로그에 알린다 */
  let audioRed = $state(false);
  /** 립싱크 보정이 바닥을 올린 뒤 끊김을 보지 않는 시간(ms): 이 값 + 올린 양 × 10(브라우저가 약 100ms/초로 따라온다) */
  const LIPSYNC_RAISE_HOLD_MS = 1000;
  /** 코덱·해상도가 바뀐 뒤, 창이 다시 보인 뒤 끊김을 보지 않는 시간(ms) */
  const STREAM_CHANGE_HOLD_MS = 2000;

  interface LipSyncDebug {
    enabled: boolean;
    adjustMs: number;
    floors: LipSyncFloors;
    /** 최근 200개. state·reason은 보정기의 불안정 판정(src/lib/lipsync.ts), concealedDelta는 직전 통계 대비 오디오 은닉 샘플 증가 */
    history: {
      t: number;
      avOffsetMs: number | null;
      adjustMs: number;
      applied: boolean;
      state: LipSyncState;
      reason: LipSyncReason | null;
      audioTargetMs: number | null;
      audioJitterMs: number | null;
      videoTargetMs: number | null;
      concealedDelta: number;
      freezeCount: number;
      /** 그때의 시청 버퍼 B */
      bufferMs: number;
    }[];
  }
  /** 개발 빌드: `window.__dongsiLipsync`(측정 하네스가 읽는다). 배포 빌드에는 없다. */
  let lipsyncDebug: LipSyncDebug | null = null;

  // 위·아래 막대 자동 숨김(D-077, src/lib/overlayIdle.ts): 재생 중 마우스를 3초 움직이지 않으면 숨기고 커서도 감춘다.
  // 통계 패널은 사용자가 켠 것이라 같이 숨기지 않는다.
  let overlayHidden = $state(false);
  const idle = new OverlayIdle({ onchange: (hidden) => (overlayHidden = hidden) });
  /** 마지막 입력이 키보드(Tab·Enter·화살표 등)였는가. 막대 안 포커스가 키보드 포커스인지 가린다. */
  let keyboardInput = false;

  const muteState = $derived({ muted, needsUnmute });
  const rows = $derived(
    stats ? statRows(stats, { dev: isDev, streamIds, lipsync: isDev ? lipsyncView : null, bufferMs, network, audioRed }) : [],
  );
  const name = $derived(info?.broadcasterName ?? "방송");

  /** 서버의 방송 정보를 읽는다. 방송 중이면 true. */
  async function loadInfo(): Promise<boolean> {
    const live = await connect(key);
    if (!live) {
      if (status === "connecting") {
        status = "error";
        message = cardFailureMessage(conn.card[key]);
      }
      // 저장한 서버가 아니면(내 방송·로컬 감지) 홈 카드가 없으니 실패 기록을 남기지 않는다.
      if (!serverById(key)) dismissCard(key);
      return false;
    }
    serverName = live.serverName;
    const found = live.broadcasts.find((b) => b.broadcasterId === broadcasterId);
    info = found ?? info;
    return found !== undefined;
  }

  /** 정보를 읽고 시청을 시작한다(처음 들어올 때, 다시 시도할 때). */
  async function begin() {
    status = "connecting";
    message = "";
    passwordError = null;
    await loadInfo();
    // loadInfo 안에서 status가 바뀔 수 있다(TypeScript는 위의 대입만 보고 좁힌다).
    if ((status as WatchStatus) !== "error") await start();
  }

  /** 끝났거나 끊긴 상태에서 같은 방송자가 다시 방송하면 다시 접속한다. */
  async function resumeIfLive() {
    if (!shouldResume(status)) return;
    if (await loadInfo()) await start();
  }

  function stop() {
    if (statsTimer) clearTimeout(statsTimer);
    statsTimer = null;
    if (peer) {
      peer.ontrack = null;
      peer.onconnectionstatechange = null;
      peer.close();
      peer = null;
    }
    if (session !== null) {
      api.stopWatch(key, session).catch(() => {});
      session = null;
    }
    prevStats = null;
    freeze = FREEZE_INITIAL;
    stats = null;
    streamIds = [];
    // 새 연결이면 보정도 처음부터(수신기와 함께 사라지니 걸어 둔 바닥도 없어진다).
    lipsync.reset();
    lastStreamSig = null;
    lastDecoded = 0;
    lastConcealed = null;
    // 시청 버퍼도 새 시청마다 처음 값에서 다시 시작한다(시청 사이에 기억하지 않는다, D-072). 처음 값은 연결 종류가 정한다(D-088).
    buffer.reset();
    bufferMs = buffer.ms;
    lastFrame = null;
    prevAudio = null;
    audioRed = false;
    network = null;
    lastNetworkAt = Number.NEGATIVE_INFINITY;
    logger.reset();
  }

  /** 연결 종류에 맞는 시작값으로 시청 버퍼를 새로 만든다(수신기에 바닥을 걸기 전, 답이 온 직후). 개발 빌드는 측정 하네스용으로 노출한다. */
  function makeBuffer(kind: NetworkPath["kind"] | null) {
    buffer = new PlayoutBuffer(bufferOff ? { baseMs: 0, maxMs: 0 } : bufferOptionsFor(kind));
    bufferMs = buffer.ms;
    // 측정 하네스가 시청 버퍼(ms, quietMs, events)를 읽는다. 배포 빌드에는 없다.
    if (isDev) (window as unknown as { __dongsiBuffer?: PlayoutBuffer }).__dongsiBuffer = buffer;
  }

  /** 개발 빌드의 측정 고리와 보정·시청 버퍼 스위치를 시청을 시작할 때마다 새로 만든다. */
  function setupLipsync() {
    lipsyncOn = false;
    bufferOff = false;
    if (!isDev) return;
    try {
      lipsyncOn = localStorage.getItem("dongsi.lipsync") === "on";
      // "dongsi.buffer"가 "off"면 시청 버퍼를 끈다(B = 0으로 고정, D-072 전 동작. 전·후 비교용).
      bufferOff = localStorage.getItem("dongsi.buffer") === "off";
    } catch {}
    makeBuffer(null);
    lipsyncView = { enabled: lipsyncOn, adjustMs: 0 };
    lipsyncDebug = { enabled: lipsyncOn, adjustMs: 0, floors: { videoMs: 0, audioMs: 0 }, history: [] };
    (window as unknown as { __dongsiLipsync?: LipSyncDebug }).__dongsiLipsync = lipsyncDebug;
  }

  /**
   * 시청을 시작한다. 비밀번호가 필요하면 영상 자리에 비밀번호 폼을 보인다.
   * 폼에서 보낸 경우(password가 있을 때)에는 폼에 머문 채 단추만 바쁨 표시를 하고,
   * 실패하면 비밀번호 오류면 폼으로, 그 밖의 오류면 오류 화면으로 간다.
   */
  async function start(password: string | null = null, remember = false) {
    const submitted = password !== null;
    stop();
    // 내 방송은 이 PC에서 재생하지 않는다(D-065: 소리가 루프백으로 다시 방송에 들어가 에코가 쌓인다).
    // 백엔드도 `ownBroadcast`로 거부하지만, 방송 정보로 알 수 있으면 연결을 만들기 전에 멈춘다.
    if (info?.mine) {
      passwordBusy = false;
      status = "own";
      return;
    }
    if (submitted) {
      passwordBusy = true;
    } else {
      status = "connecting";
      message = "";
      passwordError = null;
    }
    setupLipsync();
    const pc = new RTCPeerConnection({ bundlePolicy: "max-bundle", rtcpMuxPolicy: "require" });
    peer = pc;
    pc.addTransceiver("video", { direction: "recvonly" });
    pc.addTransceiver("audio", { direction: "recvonly" });
    const fallback = new MediaStream();
    pc.ontrack = (event) => {
      initialFloor(event.receiver);
      streamIds = [...streamIds, `${event.track.kind}:${event.streams.map((s) => s.id).join(",") || "-"}`];
      const stream = event.streams[0] ?? fallback;
      if (!event.streams[0]) fallback.addTrack(event.track);
      if (video && video.srcObject !== stream) {
        video.srcObject = stream;
        play();
      }
    };
    pc.onconnectionstatechange = () => {
      if (peer !== pc) return;
      // Wi-Fi 끊김 진단용: 연결 상태가 바뀔 때마다 한 줄
      api.watchLog(`watch-event connection state=${pc.connectionState}`).catch(() => {});
      const next = statusFromConnectionState(pc.connectionState);
      if (!next) return;
      status = next.status;
      if (next.status === "error") message = next.message;
    };

    try {
      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      // 서버는 ICE-lite라 이쪽 후보가 없어도 연결된다. 후보 수집을 기다리지 않고 바로 보낸다.
      const answer = await api.watchBroadcast(
        key,
        broadcasterId,
        password,
        remember,
        pc.localDescription?.sdp ?? offer.sdp ?? "",
      );
      if (peer !== pc) {
        api.stopWatch(key, answer.session).catch(() => {});
        return;
      }
      session = answer.session;
      // 서버로 가는 경로로 시청 버퍼의 시작값을 정한다(Wi-Fi면 60ms). 수신기에 바닥을 걸기(setRemoteDescription) 전에 만든다.
      network = answer.network ?? null;
      lastNetworkAt = performance.now();
      makeBuffer(network?.kind ?? null);
      if (submitted) {
        // 비밀번호가 받아들여졌다: 폼에서 "연결하는 중"으로 넘어간다.
        passwordBusy = false;
        passwordError = null;
        status = "connecting";
      }
      await pc.setRemoteDescription({ type: "answer", sdp: answer.answer });
      audioRed = detectAudioRed(pc, answer.answer);
      scheduleStats(pc);
    } catch (e) {
      if (peer !== pc) return;
      pc.close();
      peer = null;
      const failure = classifyStartError({ code: errorCode(e), message: errorMessage(e) }, submitted);
      status = failure.status;
      message = failure.message;
      passwordError = failure.passwordError;
      passwordBusy = false;
    }
  }

  /** 오디오 RED(D-087)를 협상했는가: 오디오 수신기의 `getParameters().codecs`로 보고, 못 읽거나 없으면 answer SDP로 본다. */
  function detectAudioRed(pc: RTCPeerConnection, answerSdp: string): boolean {
    try {
      const receiver = pc.getTransceivers().find((t) => t.receiver.track?.kind === "audio")?.receiver;
      const codecs = receiver?.getParameters().codecs;
      if (codecs !== undefined && codecs.length > 0) return audioRedFromCodecs(codecs);
    } catch {}
    return audioRedFromSdp(answerSdp);
  }

  /** 한 트랙에 걸 바닥(ms): 시청 버퍼 B + 립싱크 보정(그 트랙 쪽, 꺼져 있으면 0). */
  function floorFor(kind: "video" | "audio"): number {
    const lip = lipsyncOn ? lipsync.floors : { videoMs: 0, audioMs: 0 };
    return buffer.ms + (kind === "video" ? lip.videoMs : lip.audioMs);
  }

  /** 수신기에 최소 재생 지연(jitterBufferTarget)을 건다. 속성이 없는 브라우저에서는 조용히 무시한다. */
  function setTarget(receiver: RTCRtpReceiver, ms: number) {
    try {
      (receiver as RTCRtpReceiver & { jitterBufferTarget?: number | null }).jitterBufferTarget = ms;
    } catch {}
  }

  /**
   * 트랙이 생길 때 첫 바닥을 건다(시작할 때 립싱크 보정은 0이라 B만).
   * playoutDelayHint는 걸지 않는다. Chrome에서 jitterBufferTarget과 같은 내부 값을 쓰는 것으로 알려져 있어(미검증)
   * 0을 걸면 B를 덮을 수 있다.
   */
  function initialFloor(receiver: RTCRtpReceiver) {
    const kind = receiver.track?.kind;
    if (kind === "video" || kind === "audio") setTarget(receiver, floorFor(kind));
  }

  /** 두 수신기에 지금 바닥(B + 립싱크 보정)을 다시 건다. */
  function applyFloors(pc: RTCPeerConnection) {
    for (const receiver of pc.getReceivers()) {
      const kind = receiver.track?.kind;
      if (kind === "video" || kind === "audio") setTarget(receiver, floorFor(kind));
    }
  }

  /** B가 바뀌었으면(`next`가 null이 아니면) 립싱크 보정에 알리고 두 수신기에 다시 건다. */
  function bufferChanged(pc: RTCPeerConnection, now: number, next: number | null) {
    if (next === null) return;
    lipsync.baseChanged(now, next - bufferMs);
    bufferMs = next;
    applyFloors(pc);
  }

  /** 통계 한 번: 오디오 끊김을 보고(손실로 생긴 은닉은 빼고 늦어서 생긴 몫만), 조용했으면 버퍼를 내린다. */
  function runBuffer(pc: RTCPeerConnection, s: WatchStats, now: number) {
    const prev = prevAudio;
    const cur = {
      concealed: s.audioConcealedSamples,
      packets: s.audioPackets,
      lost: s.audioPacketsLost ?? 0,
    };
    prevAudio = cur;
    if (prev) {
      const gap = audioStall({
        concealedSamples: cur.concealed - prev.concealed,
        packetsReceived: cur.packets - prev.packets,
        packetsLost: cur.lost - prev.lost,
      });
      if (gap?.lateMs != null) bufferChanged(pc, now, buffer.stall(now, gap.lateMs, "audio"));
    }
    bufferChanged(pc, now, buffer.tick(now));
  }

  /**
   * 화면에 나온 프레임마다(rVFC): 비디오가 늦게 받아서 늦게 나왔는지 본다(playoutBuffer.ts `videoStallMs`).
   * WebView2가 rtpTimestamp를 주지 않으면 비디오로는 재지 않는다(오디오 끊김만 본다).
   */
  function onVideoFrame(now: number, meta: VideoFrameCallbackMetadata) {
    const pc = peer;
    if (!pc || meta.rtpTimestamp === undefined || document.hidden) {
      lastFrame = null;
      return;
    }
    const mark = {
      displayMs: meta.expectedDisplayTime,
      rtp: meta.rtpTimestamp,
      receiveMs: meta.receiveTime ?? null,
      width: meta.width,
      height: meta.height,
    };
    const prev = lastFrame;
    lastFrame = mark;
    if (!prev || prev.width !== mark.width || prev.height !== mark.height) return;
    const late = videoStallMs(prev, mark);
    if (late !== null) bufferChanged(pc, now, buffer.stall(now, late, "video"));
  }

  /** 창이 숨겨졌다 돌아오면 프레임 간격을 새로 잰다. */
  function onVisibility() {
    lastFrame = null;
    if (!document.hidden) buffer.hold(performance.now(), STREAM_CHANGE_HOLD_MS);
  }

  /** 통계 한 번을 보정기에 넣고, 바닥이 바뀌면 수신기에 건다. 오디오가 없으면(EPT 차이가 null) 보정기가 아무것도 하지 않는다. */
  function runLipsync(pc: RTCPeerConnection, s: WatchStats, now: number) {
    let floors: LipSyncFloors | null = null;
    // 코덱·해상도가 바뀌면(방송 중 화질 변경) 보정값은 두고 잠시 재지 않는다. 시청 버퍼도 그동안의 공백을 끊김으로 보지 않는다.
    const sig = s.codec && s.width > 0 ? `${s.codec} ${s.width}x${s.height}` : null;
    if (sig !== null) {
      if (lastStreamSig !== null && (sig !== lastStreamSig || s.framesDecoded < lastDecoded)) {
        if (lipsyncOn) lipsync.streamChanged(now);
        buffer.hold(now, STREAM_CHANGE_HOLD_MS);
        lastFrame = null;
      }
      lastStreamSig = sig;
    }
    lastDecoded = s.framesDecoded;

    if (lipsyncOn) {
      // 보정기는 B 위의 차이를 다룬다: 목표 지연·지터 버퍼 지연에서 B를 빼서 넣는다(lipsync.ts 머리말).
      const b = buffer.ms;
      const rel = (v: number | null) => (v === null ? null : v - b);
      const before = lipsync.floors;
      floors = lipsync.update({
        now,
        avOffsetMs: s.avOffsetMs,
        framesDecoded: s.framesDecoded,
        audioPackets: s.audioPackets,
        videoTargetMs: rel(s.videoTargetMs),
        audioTargetMs: rel(s.audioTargetMs),
        audioJitterMs: rel(s.audioJitterMs),
        audioConcealedSamples: s.audioConcealedSamples,
        freezeCount: s.freezeCount,
      });
      if (floors) {
        // 바닥을 올리면 그만큼 화면이 멈추거나 소리가 늘어난다: 그 공백을 끊김으로 보지 않는다.
        const up = Math.max(floors.videoMs - before.videoMs, floors.audioMs - before.audioMs, 0);
        if (up > 0) buffer.hold(now, LIPSYNC_RAISE_HOLD_MS + up * 10);
        applyFloors(pc);
      }
    }

    // 개발 빌드: 꺼져 있어도 A/V 차이 기록은 남긴다(전·후 비교).
    if (isDev) {
      const dbg = lipsync.debug;
      lipsyncView = { enabled: lipsyncOn, adjustMs: lipsync.adjustMs, state: dbg.state };
      const concealedDelta = lastConcealed === null ? 0 : s.audioConcealedSamples - lastConcealed;
      lastConcealed = s.audioConcealedSamples;
      if (lipsyncDebug) {
        lipsyncDebug.adjustMs = lipsync.adjustMs;
        lipsyncDebug.floors = lipsync.floors;
        lipsyncDebug.history.push({
          t: Math.round(now),
          avOffsetMs: s.avOffsetMs,
          adjustMs: lipsync.adjustMs,
          applied: floors !== null,
          state: dbg.state,
          reason: dbg.reason,
          audioTargetMs: s.audioTargetMs,
          audioJitterMs: s.audioJitterMs,
          videoTargetMs: s.videoTargetMs,
          concealedDelta,
          freezeCount: s.freezeCount,
          bufferMs: buffer.ms,
        });
        if (lipsyncDebug.history.length > 200) lipsyncDebug.history.shift();
      }
    }
  }

  /** 통계를 읽는 고리: 처음 15초는 500ms, 그 뒤 1초 간격(립싱크 보정이 빨리 수렴하도록). */
  function scheduleStats(pc: RTCPeerConnection) {
    if (peer !== pc) return;
    statsTimer = setTimeout(async () => {
      try {
        await updateStats();
      } catch {}
      scheduleStats(pc);
    }, lipsync.nextIntervalMs(performance.now()));
  }

  async function play() {
    if (!video) return;
    try {
      video.muted = effectiveMuted({ muted, needsUnmute });
      await video.play();
      needsUnmute = false;
    } catch {
      // 자동 재생 정책: 소리를 끄고 재생한 뒤 사용자가 켜게 한다.
      video.muted = true;
      needsUnmute = true;
      try {
        await video.play();
      } catch {}
    }
  }

  /** 음량 단추·M 키·"소리 켜기": 소리가 막혀 있어도 한 번에 켠다. */
  function onToggleMute() {
    const next = toggleMute({ muted, needsUnmute });
    muted = next.muted;
    needsUnmute = next.needsUnmute;
    if (next.unmute && video) {
      // 사용자 입력 안에서 바로 풀고 재생을 다시 시도한다.
      video.muted = false;
      video.play().catch(() => {});
    }
  }

  async function updateStats() {
    const pc = peer;
    if (!pc) return;
    const report = await pc.getStats();
    if (peer !== pc) return;
    const now = performance.now();
    const { stats: next, current } = collectStats(report, prevStats);
    prevStats = current;
    stats = next;
    runLipsync(pc, next, now);
    runBuffer(pc, next, now);
    if (isDev && now - lastDevLog >= 1900) {
      lastDevLog = now;
      api.devLog(devLogLine(next, document.hidden, buffer.ms, audioRed)).catch(() => {});
    }

    const frozen = checkFrozen(freeze, {
      framesDecoded: next.framesDecoded,
      videoKbps: next.videoKbps,
      playing: status === "playing",
      now,
    });
    freeze = frozen.state;
    const keyframeRequested = frozen.requestKeyframe && session !== null;
    if (keyframeRequested) {
      api.requestKeyframe(key, session!).catch(() => {});
    }

    for (const line of logger.update({
      now,
      stats: next,
      bufferMs: buffer.ms,
      hidden: document.hidden,
      network,
      keyframeRequested,
      audioRed,
    })) {
      api.watchLog(line).catch(() => {});
    }
    refreshNetwork(pc, now);
  }

  /** 10초마다 서버로 가는 경로를 다시 읽는다(Wi-Fi 신호는 변한다). 기다리지 않고, 못 읽으면 이전 값을 둔다. */
  function refreshNetwork(pc: RTCPeerConnection, now: number) {
    if (now - lastNetworkAt < NETWORK_REFRESH_MS) return;
    lastNetworkAt = now;
    api
      .networkPath(key)
      .then((n) => {
        if (peer === pc && n) network = n;
      })
      .catch(() => {});
  }

  function submitPassword(password: string, remember: boolean) {
    void start(password, remember);
  }

  /** 새 창으로 옮긴다. 이 화면은 이전 화면으로 돌아간다. */
  async function popOut() {
    try {
      await api.openWatchWindow(key, broadcasterId, info?.broadcasterName ?? "");
      stop();
      location.hash = backHref.slice(1);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  function closeWindow() {
    void getCurrentWindow().close();
  }

  function toggleFullscreen() {
    if (document.fullscreenElement) document.exitFullscreen();
    else root?.requestFullscreen().catch(() => {});
  }

  function onKey(event: KeyboardEvent) {
    // 키보드로 막대를 다루면 막대 안 포커스를 키보드 포커스로 보고 숨기지 않는다. 단축키(M·F)는 세지 않는다.
    if (isOverlayKey(event)) {
      keyboardInput = true;
      if (inOverlay(event.target)) idle.hold("focus", true);
    }
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const target = event.target;
    if (
      target instanceof HTMLElement &&
      isTypingTarget(target.tagName, target instanceof HTMLInputElement ? target.type : null)
    ) {
      return;
    }
    if (event.key === "f" || event.key === "F") toggleFullscreen();
    if ((event.key === "m" || event.key === "M") && !event.repeat) onToggleMute();
  }

  /** 위쪽 막대·조작 막대(.w-overlay) 안의 요소인가 */
  function inOverlay(target: EventTarget | null): boolean {
    return target instanceof Element && target.closest(".w-overlay") !== null;
  }

  function onPointerMove(event: PointerEvent) {
    idle.pointerMoved(event.clientX, event.clientY);
  }

  /** 마우스가 막대 위에 있는가(pointerover는 요소 경계를 넘을 때마다 온다. 숨긴 막대는 pointer-events가 없어 오지 않는다). */
  function onPointerOver(event: PointerEvent) {
    idle.hold("pointer", inOverlay(event.target));
  }

  function onPointerLeave() {
    idle.hold("pointer", false);
  }

  /** 누르면 마우스 입력으로 돌아간다: 키보드 포커스 이유를 풀고 다시 잰다(마우스로 누른 단추에 포커스가 남아도 숨긴다). */
  function onPointerDown() {
    keyboardInput = false;
    idle.hold("focus", false);
    idle.activity();
  }

  function onFocusIn(event: FocusEvent) {
    idle.hold("focus", keyboardInput && inOverlay(event.target));
  }

  function onFocusOut(event: FocusEvent) {
    if (!inOverlay(event.relatedTarget)) idle.hold("focus", false);
  }

  /** 영상을 더블 클릭하면 전체 화면(막대·단추 위는 제외) */
  function onDblClick(event: MouseEvent) {
    if (status !== "playing") return;
    if (event.target instanceof Element && event.target.closest("button, a, input, label, .w-panel")) return;
    toggleFullscreen();
  }

  // 화면 프레임 콜백(rVFC) 고리: 시청 버퍼의 비디오 끊김 감지. video 요소는 시청을 다시 시작해도 그대로라 요소마다 한 번 건다.
  $effect(() => {
    const v = video;
    if (!v || typeof v.requestVideoFrameCallback !== "function") return;
    let id = 0;
    const onFrame = (now: number, meta: VideoFrameCallbackMetadata) => {
      id = v.requestVideoFrameCallback(onFrame);
      onVideoFrame(now, meta);
    };
    id = v.requestVideoFrameCallback(onFrame);
    return () => v.cancelVideoFrameCallback(id);
  });

  $effect(() => {
    idle.setEnabled(status === "playing");
  });

  $effect(() => {
    if (video) {
      video.volume = volume;
      video.muted = effectiveMuted({ muted, needsUnmute });
    }
  });

  onMount(async () => {
    const handlers = await Promise.all([
      listen<BroadcastEndedEvent>("broadcast-ended", (e) => {
        if (e.payload.key === key && e.payload.broadcasterId === broadcasterId) {
          stop();
          passwordBusy = false;
          status = "ended";
        }
      }),
      listen<ServerClosedEvent>("server-closed", (e) => {
        if (e.payload.key === key && status !== "ended") {
          stop();
          passwordBusy = false;
          status = "error";
          message = e.payload.reason;
        }
      }),
      listen<KeyEvent>("server-changed", (e) => {
        if (e.payload.key !== key) return;
        if (shouldResume(status)) void resumeIfLive();
        else void loadInfo();
      }),
      // 이미 열린 시청 창을 다시 열려고 하면 백엔드가 보낸다.
      listen("watch-reopen", () => void resumeIfLive()),
    ]);
    if (destroyed) {
      for (const u of handlers) u();
      return;
    }
    unlisten.push(...handlers);
    await begin();
  });

  onDestroy(() => {
    destroyed = true;
    idle.dispose();
    stop();
    for (const u of unlisten) u();
  });
</script>

<svelte:window onkeydown={onKey} />
<svelte:document onvisibilitychange={onVisibility} />

<div
  class="root"
  class:idle={overlayHidden}
  bind:this={root}
  ondblclick={onDblClick}
  onpointermove={onPointerMove}
  onpointerover={onPointerOver}
  onpointerleave={onPointerLeave}
  onpointerdown={onPointerDown}
  onfocusin={onFocusIn}
  onfocusout={onFocusOut}
  role="presentation"
>
  <video bind:this={video} class:shown={status === "playing"} autoplay playsinline aria-label="방송 영상"></video>

  {#if status !== "playing"}
    <StageBackdrop {hue} dim={status === "ended" || status === "error" || status === "own"}>
      {#if status === "password"}
        <PasswordForm
          {name}
          {broadcasterId}
          subtitle={passwordSubtitle(name, info)}
          error={passwordError}
          busy={passwordBusy}
          onsubmit={submitPassword}
        />
      {:else}
        <StatusScreen
          kind={status}
          {name}
          {message}
          {popout}
          {backHref}
          onretry={begin}
          onclose={closeWindow}
        />
      {/if}
    </StageBackdrop>
  {/if}

  <WatchTopBar
    {popout}
    {backHref}
    {name}
    live={showLive(status, info !== null)}
    {serverName}
    viewers={info && showInfoBar(status, true) ? info.viewers : null}
    hidden={overlayHidden}
    onclose={closeWindow}
  />

  {#if showStats && stats && status === "playing"}
    <WatchStatsPanel {rows} />
  {/if}

  {#if status === "playing"}
    <WatchControls
      bind:volume
      mute={muteState}
      {showStats}
      {popout}
      hidden={overlayHidden}
      ontogglemute={onToggleMute}
      ontogglestats={() => (showStats = !showStats)}
      onpopout={popOut}
      onfullscreen={toggleFullscreen}
      onvolumedrag={(dragging) => idle.hold("drag", dragging)}
    />
  {/if}
</div>

<style>
  /*
   * 영상 둘레 바탕은 순수한 검정이 아니라 앱 바탕색(--bg)으로 둔다. WebView2에서 영상과 순수한 검은 바탕만으로 창이 채워지면
   * 검은 띠 위의 오버레이가 화면에 나오지 않는다(페이지 안에서는 보이고 눌린다, MicrosoftEdge/WebView2Feedback#5574).
   */
  .root {
    position: relative;
    flex: 1;
    min-height: 100vh;
    background: var(--bg);
    overflow: hidden;
  }

  /* 막대를 숨기면 영상 위 커서도 감춘다(D-077) */
  .root.idle {
    cursor: none;
  }

  video {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: var(--bg);
    visibility: hidden;
  }

  video.shown {
    visibility: visible;
  }
</style>
