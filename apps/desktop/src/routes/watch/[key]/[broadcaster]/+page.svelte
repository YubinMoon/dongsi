<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { page } from "$app/state";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, codecLabel, errorCode, errorMessage } from "$lib/api";
  import type {
    BroadcastEndedEvent,
    BroadcastView,
    KeyEvent,
    ServerClosedEvent,
  } from "$lib/types";
  import PasswordDialog from "$lib/components/PasswordDialog.svelte";
  import { collectStats, type WatchStats } from "$lib/watchStats";

  const key = $derived(page.params.key ?? "");
  const broadcasterId = $derived(page.params.broadcaster ?? "");
  const popout = typeof location !== "undefined" && location.hash.includes("popout=1");
  const backHref = $derived(
    key === "host" ? "#/broadcast" : key.startsWith("local-") ? "#/" : `#/server/${key}`,
  );

  type Status = "connecting" | "playing" | "password" | "ended" | "error";

  let status = $state<Status>("connecting");
  let message = $state("");
  let info = $state<BroadcastView | null>(null);
  let serverName = $state("");
  let stats = $state<WatchStats | null>(null);
  let showStats = $state(true);
  let volume = $state(1);
  let muted = $state(false);
  let needsUnmute = $state(false);
  let passwordOpen = $state(false);

  let video: HTMLVideoElement | undefined = $state();
  let stage: HTMLDivElement | undefined = $state();

  let pc: RTCPeerConnection | null = null;
  let session: number | null = null;
  let statsTimer: ReturnType<typeof setInterval> | null = null;
  let unlisten: UnlistenFn[] = [];
  let lastFramesDecoded = 0;
  let frozenSince: number | null = null;
  let lastKeyframeRequest = 0;
  let prevStats: Map<string, RTCStats> | null = null;
  const isDev = import.meta.env.DEV;
  /** 개발용: 트랙별 MediaStream ID(같아야 브라우저가 A/V를 맞춘다) */
  let streamIds = $state<string[]>([]);
  let statsTick = 0;

  async function loadInfo() {
    try {
      const live = await api.connectServer(key);
      serverName = live.serverName;
      info = live.broadcasts.find((b) => b.broadcasterId === broadcasterId) ?? info;
    } catch (e) {
      if (status === "connecting") {
        status = "error";
        message = errorMessage(e);
      }
    }
  }

  function stop() {
    if (statsTimer) clearInterval(statsTimer);
    statsTimer = null;
    if (pc) {
      pc.ontrack = null;
      pc.onconnectionstatechange = null;
      pc.close();
      pc = null;
    }
    if (session !== null) {
      api.stopWatch(key, session).catch(() => {});
      session = null;
    }
    prevStats = null;
    stats = null;
    streamIds = [];
  }

  /** 시청을 시작한다. 비밀번호가 필요하면 대화 상자를 띄운다(대화 상자에서 부를 때는 오류를 던진다). */
  async function start(password: string | null = null, remember = false, fromDialog = false) {
    stop();
    status = "connecting";
    message = "";
    const conn = new RTCPeerConnection({ bundlePolicy: "max-bundle", rtcpMuxPolicy: "require" });
    pc = conn;
    conn.addTransceiver("video", { direction: "recvonly" });
    conn.addTransceiver("audio", { direction: "recvonly" });
    const fallback = new MediaStream();
    conn.ontrack = (event) => {
      lowLatency(event.receiver);
      streamIds = [...streamIds, `${event.track.kind}:${event.streams.map((s) => s.id).join(",") || "-"}`];
      const stream = event.streams[0] ?? fallback;
      if (!event.streams[0]) fallback.addTrack(event.track);
      if (video && video.srcObject !== stream) {
        video.srcObject = stream;
        play();
      }
    };
    conn.onconnectionstatechange = () => {
      if (pc !== conn) return;
      if (conn.connectionState === "connected") {
        status = "playing";
      } else if (conn.connectionState === "failed") {
        status = "error";
        message = "미디어 연결에 실패했습니다. 방화벽에서 UDP가 막혀 있는지 확인해 주세요.";
      }
    };

    try {
      const offer = await conn.createOffer();
      await conn.setLocalDescription(offer);
      // 서버는 ICE-lite라 이쪽 후보가 없어도 연결된다. 후보 수집을 기다리지 않고 바로 보낸다.
      const answer = await api.watchBroadcast(
        key,
        broadcasterId,
        password,
        remember,
        conn.localDescription?.sdp ?? offer.sdp ?? "",
      );
      if (pc !== conn) {
        api.stopWatch(key, answer.session).catch(() => {});
        return;
      }
      session = answer.session;
      await conn.setRemoteDescription({ type: "answer", sdp: answer.answer });
      statsTimer = setInterval(updateStats, 1000);
    } catch (e) {
      if (pc === conn) {
        conn.close();
        pc = null;
      }
      const code = errorCode(e);
      if (code === "passwordRequired" || code === "wrongPassword") {
        if (fromDialog) throw e;
        status = "password";
        message = errorMessage(e);
        passwordOpen = true;
        return;
      }
      if (fromDialog) throw e;
      status = code === "notFound" ? "ended" : "error";
      message = code === "notFound" ? "방송이 끝났거나 찾을 수 없습니다." : errorMessage(e);
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
      video.muted = muted;
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

  function unmute() {
    muted = false;
    needsUnmute = false;
    if (video) {
      video.muted = false;
      video.play().catch(() => {});
    }
  }

  async function updateStats() {
    if (!pc) return;
    const report = await pc.getStats();
    const { stats: next, current } = collectStats(report, prevStats);
    prevStats = current;
    stats = next;
    if (isDev && ++statsTick % 2 === 0) {
      const r = (n: number | null) => (n === null ? "-" : n.toFixed(0));
      api
        .devLog(
          `watch ${next.width}x${next.height} fps=${r(next.fps)} kbps=${r(next.videoKbps)}+${r(next.audioKbps)} ` +
            `latency=${r(next.latencyMs)} av=${r(next.avOffsetMs)} jb=${r(next.videoJitterMs)}/${r(next.audioJitterMs)} ` +
            `target=${r(next.videoTargetMs)}/${r(next.audioTargetMs)} lost=${next.packetsLost} dropped=${next.framesDropped} ` +
            `freeze=${next.freezeCount} codec=${next.codec} hidden=${document.hidden}`,
        )
        .catch(() => {});
    }

    // 화면이 멈췄는데 패킷은 오면 키프레임을 요청한다(브라우저도 PLI를 보내지만 한 번 더).
    const now = performance.now();
    if (next.framesDecoded > lastFramesDecoded) {
      lastFramesDecoded = next.framesDecoded;
      frozenSince = null;
    } else if (status === "playing" && next.videoKbps > 0) {
      frozenSince ??= now;
      if (now - frozenSince > 2000 && now - lastKeyframeRequest > 2000 && session !== null) {
        lastKeyframeRequest = now;
        api.requestKeyframe(key, session).catch(() => {});
      }
    }
  }

  async function submitPassword(password: string, remember: boolean) {
    await start(password, remember, true);
  }

  /** 새 창으로 옮긴다. 이 화면은 이전 화면으로 돌아간다. */
  async function popOut() {
    try {
      await api.openWatchWindow(key, broadcasterId, info?.broadcasterName ?? "");
      stop();
      location.hash = backHref.slice(1);
    } catch (e) {
      message = errorMessage(e);
    }
  }

  function toggleFullscreen() {
    if (document.fullscreenElement) document.exitFullscreen();
    else stage?.requestFullscreen().catch(() => {});
  }

  function onKey(event: KeyboardEvent) {
    if (event.target instanceof HTMLInputElement) return;
    if (event.key === "f" || event.key === "F") toggleFullscreen();
    if (event.key === "m" || event.key === "M") {
      muted = !muted;
      if (!muted) unmute();
    }
  }

  $effect(() => {
    if (video) {
      video.volume = volume;
      video.muted = muted || needsUnmute;
    }
  });

  onMount(async () => {
    unlisten.push(
      await listen<BroadcastEndedEvent>("broadcast-ended", (e) => {
        if (e.payload.key === key && e.payload.broadcasterId === broadcasterId) {
          stop();
          status = "ended";
          message = "방송이 끝났습니다.";
        }
      }),
      await listen<ServerClosedEvent>("server-closed", (e) => {
        if (e.payload.key === key && status !== "ended") {
          stop();
          status = "error";
          message = e.payload.reason;
        }
      }),
      await listen<KeyEvent>("server-changed", (e) => {
        if (e.payload.key === key) loadInfo();
      }),
    );
    await loadInfo();
    if (status !== "error") await start();
  });

  onDestroy(() => {
    stop();
    for (const u of unlisten) u();
  });

  function fmt(n: number | null | undefined, digits = 0): string {
    return n === null || n === undefined || Number.isNaN(n) ? "–" : n.toFixed(digits);
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="top">
  {#if popout}
    <button class="ghost small" onclick={() => getCurrentWindow().close()}>창 닫기</button>
  {:else}
    <a class="back muted" href={backHref}>← 돌아가기</a>
  {/if}
  <div class="title">
    <h1>{info?.broadcasterName ?? "방송"}</h1>
    <span class="muted small">{serverName}</span>
  </div>
  <span class="spacer"></span>
  {#if !popout}
    <button class="ghost small" onclick={popOut}>새 창으로 보기</button>
  {/if}
  <button class="ghost small" onclick={() => (showStats = !showStats)}>
    {showStats ? "통계 숨기기" : "통계 보기"}
  </button>
</div>

<div class="stage" bind:this={stage} ondblclick={toggleFullscreen} role="presentation">
  <!-- svelte-ignore a11y_media_has_caption -->
  <video bind:this={video} autoplay playsinline></video>

  {#if status !== "playing"}
    <div class="overlay">
      {#if status === "connecting"}
        <p>연결하는 중…</p>
      {:else if status === "password"}
        <p>비밀번호가 필요한 방송입니다.</p>
        <button class="primary" onclick={() => (passwordOpen = true)}>비밀번호 입력</button>
      {:else if status === "ended"}
        <p>{message || "방송이 끝났습니다."}</p>
        {#if popout}
          <button onclick={() => getCurrentWindow().close()}>창 닫기</button>
        {:else}
          <a class="button" href={backHref}>목록으로</a>
        {/if}
      {:else}
        <p class="error">{message}</p>
        <button class="primary" onclick={() => start()}>다시 시도</button>
      {/if}
    </div>
  {/if}

  {#if needsUnmute && status === "playing"}
    <button class="unmute primary" onclick={unmute}>소리 켜기</button>
  {/if}

  {#if showStats && stats && status === "playing"}
    <dl class="stats mono">
      <dt>해상도</dt><dd>{stats.width}×{stats.height} · {fmt(stats.fps)}fps</dd>
      <dt>코덱</dt><dd>{stats.codec ?? "–"}{stats.decoder ? ` (${stats.decoder})` : ""}</dd>
      <dt>비트레이트</dt><dd>{fmt(stats.videoKbps)} + {fmt(stats.audioKbps)} kbps</dd>
      <dt>지연</dt><dd>{fmt(stats.latencyMs)} ms</dd>
      <dt>A/V 차이</dt><dd>{fmt(stats.avOffsetMs)} ms</dd>
      <dt>지터 버퍼</dt><dd>{fmt(stats.videoJitterMs)} / {fmt(stats.audioJitterMs)} ms</dd>
      <dt>RTT</dt><dd>{fmt(stats.rttMs)} ms</dd>
      <dt>손실</dt><dd>{stats.packetsLost} · 버린 프레임 {stats.framesDropped} · 멈춤 {stats.freezeCount}</dd>
      {#if isDev}
        <dt>목표 지연</dt><dd>{fmt(stats.videoTargetMs)} / {fmt(stats.audioTargetMs)} ms</dd>
        <dt>스트림</dt><dd>{streamIds.join(" ")}</dd>
      {/if}
    </dl>
  {/if}
</div>

<div class="controls">
  <label class="volume">
    <span>음량</span>
    <input type="range" min="0" max="1" step="0.01" bind:value={volume} disabled={muted} />
  </label>
  <button class="ghost small" onclick={() => (muted ? unmute() : (muted = true))}>
    {muted ? "소리 켜기" : "음소거"}
  </button>
  <span class="spacer"></span>
  {#if info}
    <span class="muted small">
      {codecLabel(info.codec)} · {info.width}×{info.height} · {info.fps}fps · 시청자 {info.viewers}명
    </span>
  {/if}
  <button class="small" onclick={toggleFullscreen}>전체 화면 (F)</button>
</div>

<PasswordDialog
  bind:open={passwordOpen}
  broadcasterName={info?.broadcasterName ?? ""}
  onsubmit={submitPassword}
/>

<style>
  .top {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 0.8rem;
  }

  .title {
    display: grid;
  }

  .back:hover {
    color: var(--text);
  }

  .stage {
    position: relative;
    background: #000;
    border-radius: var(--radius);
    overflow: hidden;
    aspect-ratio: 16 / 9;
    max-height: calc(100vh - 13rem);
    width: 100%;
    display: grid;
    place-items: center;
  }

  .stage:fullscreen {
    border-radius: 0;
    max-height: none;
    aspect-ratio: auto;
  }

  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #000;
  }

  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 0.8rem;
    background: rgb(0 0 0 / 0.6);
    text-align: center;
    padding: 1rem;
  }

  .overlay p {
    margin: 0;
  }

  .unmute {
    position: absolute;
    bottom: 1rem;
    left: 50%;
    transform: translateX(-50%);
  }

  .stats {
    position: absolute;
    top: 0.6rem;
    left: 0.6rem;
    margin: 0;
    padding: 0.5rem 0.7rem;
    display: grid;
    grid-template-columns: auto auto;
    gap: 0.1rem 0.8rem;
    background: rgb(0 0 0 / 0.65);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    color: #dfe3ea;
    pointer-events: none;
  }

  .stats dt {
    color: #9aa3b2;
  }

  .stats dd {
    margin: 0;
  }

  .controls {
    margin-top: 0.7rem;
    display: flex;
    align-items: center;
    gap: 0.8rem;
  }

  .volume {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.9rem;
  }

  .volume input {
    width: 140px;
  }

  .button {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.45rem 0.9rem;
    background: var(--bg-elev);
  }
</style>
