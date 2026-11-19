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
  import type { BroadcastEndedEvent, BroadcastView, KeyEvent, ServerClosedEvent } from "$lib/types";
  import {
    backHref as backHrefFor,
    cardFailureMessage,
    checkFrozen,
    classifyStartError,
    devLogLine,
    effectiveMuted,
    FREEZE_INITIAL,
    infoLine,
    isPopoutHash,
    isTypingTarget,
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
  import { collectStats, type WatchStats } from "$lib/watchStats";
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
  let showStats = $state(true);
  let volume = $state(1);
  let muted = $state(false);
  let needsUnmute = $state(false);

  let video: HTMLVideoElement | undefined = $state();
  let root: HTMLDivElement | undefined = $state();

  let peer: RTCPeerConnection | null = null;
  let session: number | null = null;
  let statsTimer: ReturnType<typeof setInterval> | null = null;
  let unlisten: UnlistenFn[] = [];
  let destroyed = false;
  let freeze: FreezeState = FREEZE_INITIAL;
  let prevStats: Map<string, RTCStats> | null = null;
  const isDev = import.meta.env.DEV;
  /** 개발용: 트랙별 MediaStream ID(같아야 브라우저가 A/V를 맞춘다) */
  let streamIds = $state<string[]>([]);
  let statsTick = 0;

  const muteState = $derived({ muted, needsUnmute });
  const rows = $derived(stats ? statRows(stats, { dev: isDev, streamIds }) : []);
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
    if (statsTimer) clearInterval(statsTimer);
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
  }

  /**
   * 시청을 시작한다. 비밀번호가 필요하면 영상 자리에 비밀번호 폼을 보인다.
   * 폼에서 보낸 경우(password가 있을 때)에는 폼에 머문 채 단추만 바쁨 표시를 하고,
   * 실패하면 비밀번호 오류면 폼으로, 그 밖의 오류면 오류 화면으로 간다.
   */
  async function start(password: string | null = null, remember = false) {
    const submitted = password !== null;
    stop();
    if (submitted) {
      passwordBusy = true;
    } else {
      status = "connecting";
      message = "";
      passwordError = null;
    }
    const pc = new RTCPeerConnection({ bundlePolicy: "max-bundle", rtcpMuxPolicy: "require" });
    peer = pc;
    pc.addTransceiver("video", { direction: "recvonly" });
    pc.addTransceiver("audio", { direction: "recvonly" });
    const fallback = new MediaStream();
    pc.ontrack = (event) => {
      lowLatency(event.receiver);
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
      if (submitted) {
        // 비밀번호가 받아들여졌다: 폼에서 "연결하는 중"으로 넘어간다.
        passwordBusy = false;
        passwordError = null;
        status = "connecting";
      }
      await pc.setRemoteDescription({ type: "answer", sdp: answer.answer });
      statsTimer = setInterval(updateStats, 1000);
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

  /** 지연을 줄인다: 지터 버퍼 목표를 최소로. */
  function lowLatency(receiver: RTCRtpReceiver) {
    try {
      (receiver as RTCRtpReceiver & { jitterBufferTarget?: number }).jitterBufferTarget = 0;
    } catch {}
    try {
      (receiver as RTCRtpReceiver & { playoutDelayHint?: number }).playoutDelayHint = 0;
    } catch {}
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
    const { stats: next, current } = collectStats(report, prevStats);
    prevStats = current;
    stats = next;
    if (isDev && ++statsTick % 2 === 0) {
      api.devLog(devLogLine(next, document.hidden)).catch(() => {});
    }

    const frozen = checkFrozen(freeze, {
      framesDecoded: next.framesDecoded,
      videoKbps: next.videoKbps,
      playing: status === "playing",
      now: performance.now(),
    });
    freeze = frozen.state;
    if (frozen.requestKeyframe && session !== null) {
      api.requestKeyframe(key, session).catch(() => {});
    }
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

  /** 영상을 더블 클릭하면 전체 화면(막대·단추 위는 제외) */
  function onDblClick(event: MouseEvent) {
    if (status !== "playing") return;
    if (event.target instanceof Element && event.target.closest("button, a, input, label, .w-panel")) return;
    toggleFullscreen();
  }

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
    stop();
    for (const u of unlisten) u();
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="root" bind:this={root} ondblclick={onDblClick} role="presentation">
  <video bind:this={video} class:shown={status === "playing"} autoplay playsinline aria-label="방송 영상"></video>

  {#if status !== "playing"}
    <StageBackdrop {hue} dim={status === "ended" || status === "error"}>
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
    infoText={info && showInfoBar(status, true) ? infoLine(info) : null}
    viewers={info?.viewers ?? null}
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
      ontogglemute={onToggleMute}
      ontogglestats={() => (showStats = !showStats)}
      onpopout={popOut}
      onfullscreen={toggleFullscreen}
    />
  {/if}
</div>

<style>
  .root {
    position: relative;
    flex: 1;
    min-height: 100vh;
    background: #000000;
    overflow: hidden;
  }

  video {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #000000;
    visibility: hidden;
  }

  video.shown {
    visibility: visible;
  }
</style>
