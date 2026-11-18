<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { api, codecLabel, errorMessage } from "$lib/api";
  import { toast } from "$lib/toast.svelte";
  import type {
    AudioMode,
    BroadcastStatus,
    EncoderView,
    HostingKind,
    PresetView,
    ServerView,
    SourceView,
    VideoCodec,
    VideoSettings,
  } from "$lib/types";

  const CODECS: VideoCodec[] = ["h264", "h265", "vp8", "vp9", "av1"];
  const MIME: Record<VideoCodec, string> = {
    h264: "video/h264",
    h265: "video/h265",
    vp8: "video/vp8",
    vp9: "video/vp9",
    av1: "video/av1",
  };
  const AUDIO_LABELS: Record<AudioMode, string> = {
    systemExceptDiscord: "시스템 소리 (Discord 제외)",
    app: "캡처하는 앱의 소리만",
    system: "시스템 소리 전체 (Discord 포함)",
    none: "소리 없음",
    testTone: "테스트 톤 (A/V 동기 측정)",
  };
  const isDev = import.meta.env.DEV;
  /** 기본 포트(D-028). 내장 서버가 다른 포트로 열리면 자동 감지에 안 나타난다. */
  const DEFAULT_PORT = 7884;

  let status = $state<BroadcastStatus | null>(null);
  let loaded = $state(false);

  // 설정 폼
  let servers = $state<ServerView[]>([]);
  let sources = $state<SourceView[]>([]);
  let encoders = $state<EncoderView[] | null>(null);
  let presets = $state<PresetView[]>([]);
  let receivable = $state<Set<string>>(new Set());

  let hosting = $state<HostingKind>("lan");
  let serverId = $state<string>("");
  let sourceId = $state<string>("");
  let presetId = $state<string>("p1080p60");
  let codec = $state<VideoCodec>("h264");
  let bitrate = $state(7000);
  let bitrateTouched = $state(false);
  let audio = $state<AudioMode>("systemExceptDiscord");
  let password = $state("");
  let busy = $state(false);
  let formError = $state<string | null>(null);

  // 방송 중 설정 변경
  let livePreset = $state("");
  let liveCodec = $state<VideoCodec>("h264");
  let liveBitrate = $state(0);
  let applying = $state(false);

  let timer: ReturnType<typeof setInterval> | null = null;
  let unlisten: UnlistenFn | null = null;

  const preset = $derived(presets.find((p) => p.id === presetId) ?? null);
  const selectedSource = $derived(sources.find((s) => s.id === sourceId) ?? null);

  function codecUsable(c: VideoCodec): { ok: boolean; why: string } {
    const enc = encoders?.find((e) => e.codec === c);
    if (encoders && !enc) return { ok: false, why: "인코더 없음" };
    if (receivable.size > 0 && !receivable.has(MIME[c])) return { ok: false, why: "시청 앱에서 재생 불가" };
    return { ok: true, why: enc ? `${enc.name}${enc.hardware ? " · 하드웨어" : ""}` : "" };
  }

  function settingsFor(p: PresetView | null, c: VideoCodec, kbps: number): VideoSettings {
    return {
      codec: c,
      width: p?.width ?? 1920,
      height: p?.height ?? 1080,
      fps: p?.fps ?? 60,
      bitrateKbps: kbps,
    };
  }

  // 프리셋·코덱을 바꾸면 비트레이트 기본값을 따라간다(직접 고치기 전까지).
  $effect(() => {
    if (preset && !bitrateTouched) bitrate = preset.bitrates[codec];
  });

  async function loadSources() {
    try {
      sources = await api.listCaptureSources();
      if (!sources.some((s) => s.id === sourceId)) {
        sourceId = (sources.find((s) => s.kind === "monitor" && s.primary) ?? sources[0])?.id ?? "";
      }
    } catch (e) {
      formError = errorMessage(e);
    }
  }

  function readReceivable() {
    try {
      const caps = RTCRtpReceiver.getCapabilities("video");
      receivable = new Set((caps?.codecs ?? []).map((c) => c.mimeType.toLowerCase()));
    } catch {
      receivable = new Set();
    }
  }

  async function refreshStatus() {
    try {
      const next = await api.broadcastStatus();
      if (next && !status) {
        livePreset = presetIdFor(next.settings);
        liveCodec = next.settings.codec;
        liveBitrate = next.settings.bitrateKbps;
      }
      status = next;
    } catch {}
  }

  function presetIdFor(s: VideoSettings): string {
    return (
      presets.find((p) => p.width === s.width && p.height === s.height && p.fps === s.fps)?.id ??
      presets[0]?.id ??
      ""
    );
  }

  async function start() {
    formError = null;
    if (!sourceId) {
      formError = "캡처할 화면이나 창을 골라 주세요";
      return;
    }
    if (hosting === "server" && !serverId) {
      formError = "방송할 서버를 골라 주세요";
      return;
    }
    busy = true;
    try {
      status = await api.startBroadcast({
        hosting,
        serverId: hosting === "server" ? serverId : null,
        source: sourceId,
        settings: settingsFor(preset, codec, bitrate),
        audio,
        password: password || null,
      });
      livePreset = presetId;
      liveCodec = codec;
      liveBitrate = bitrate;
      toast("방송을 시작했습니다", "success");
    } catch (e) {
      formError = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  /** 개발용: 테스트 패턴·테스트 톤으로 바로 방송한다(A/V 동기·지연 측정). */
  function startTest() {
    const test = sources.find((s) => s.kind === "test");
    if (!test) {
      formError = "테스트 패턴이 없습니다";
      return;
    }
    sourceId = test.id;
    audio = "testTone";
    start();
  }

  async function stop() {
    busy = true;
    try {
      await api.stopBroadcast();
      status = null;
      toast("방송을 끝냈습니다");
      loadSources();
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      busy = false;
    }
  }

  async function applyLive() {
    const p = presets.find((x) => x.id === livePreset) ?? null;
    applying = true;
    try {
      status = await api.updateBroadcast(settingsFor(p, liveCodec, liveBitrate));
      toast("방송 설정을 바꿨습니다", "success");
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      applying = false;
    }
  }

  function onLivePreset() {
    const p = presets.find((x) => x.id === livePreset);
    if (p) liveBitrate = p.bitrates[liveCodec];
  }

  function onLiveCodec() {
    const p = presets.find((x) => x.id === livePreset);
    if (p) liveBitrate = p.bitrates[liveCodec];
  }

  async function copy(text: string, what: string) {
    try {
      await navigator.clipboard.writeText(text);
      toast(`${what}를 복사했습니다`, "success");
    } catch {
      toast("복사하지 못했습니다", "error");
    }
  }

  function elapsed(startedAt: number): string {
    const s = Math.max(0, Math.floor(Date.now() / 1000 - startedAt));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}` : `${m}:${String(sec).padStart(2, "0")}`;
  }

  const HOSTING_LABEL: Record<HostingKind, string> = {
    server: "추가한 서버",
    lan: "로컬 랜",
    direct: "PC 직접 호스팅",
  };

  onMount(async () => {
    const query = new URLSearchParams(location.hash.split("?")[1] ?? "");
    readReceivable();
    try {
      [servers, presets] = await Promise.all([api.listServers(), api.qualityPresets()]);
    } catch (e) {
      formError = errorMessage(e);
    }
    const qs = query.get("server");
    if (qs && servers.some((s) => s.id === qs)) {
      hosting = "server";
      serverId = qs;
    } else if (servers.length > 0) {
      serverId = servers[0].id;
    }
    await refreshStatus();
    loaded = true;
    loadSources();
    api
      .availableEncoders()
      .then((list) => {
        encoders = list;
        if (!codecUsable(codec).ok) codec = CODECS.find((c) => codecUsable(c).ok) ?? codec;
      })
      .catch(() => (encoders = []));
    timer = setInterval(() => {
      if (status) refreshStatus();
    }, 1000);
    unlisten = await listen<{ reason: string }>("broadcast-stopped", (e) => {
      status = null;
      toast(`방송이 끝났습니다: ${e.payload.reason}`, "error");
      loadSources();
    });
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
    unlisten?.();
  });

  /** "Discord 제외" 모드에서만 Discord를 실제로 빼고 있는지 알린다(실행 중이 아니면 3초마다 다시 찾는다). */
  function discordNote(mode: AudioMode, excluded: boolean): string {
    if (mode !== "systemExceptDiscord") return "";
    return excluded ? " · Discord 소리 빼는 중" : " · Discord가 실행 중이 아님(켜지면 자동으로 뺌)";
  }

  function fmt(n: number | null | undefined, digits = 0): string {
    return n === null || n === undefined || Number.isNaN(n) ? "–" : n.toFixed(digits);
  }
</script>

<a class="back muted" href="#/">← 서버 목록</a>

{#if !loaded}
  <p class="muted">불러오는 중…</p>
{:else if status}
  <!-- 방송 중 -->
  <div class="live-head">
    <span class="live-dot" aria-hidden="true"></span>
    <h1>방송 중</h1>
    <span class="muted">{elapsed(status.startedAt)} · 시청자 {status.viewers}명</span>
    <span class="spacer"></span>
    <button onclick={() => api.openWatchWindow(status!.key, status!.broadcasterId, "내 방송 미리 보기").catch((e) => toast(errorMessage(e), "error"))}>
      미리 보기(새 창)
    </button>
    <a class="button" href="#/watch/{status.key}/{status.broadcasterId}">내 방송 미리 보기</a>
    <button class="danger solid" onclick={stop} disabled={busy}>방송 끝내기</button>
  </div>

  <div class="grid">
    <section class="card pane">
      <h2>공유</h2>
      <p class="muted small">
        {HOSTING_LABEL[status.hosting]} · {status.serverName}{status.port ? ` · 포트 ${status.port}` : ""}
      </p>
      {#if status.port && status.port !== DEFAULT_PORT}
        <p class="small warn-text">
          {DEFAULT_PORT}번 포트를 다른 프로그램(같은 PC의 전용 서버 등)이 쓰고 있어 {status.port}번으로 열었습니다. 이
          방송은 같은 LAN·Tailscale의 자동 감지 목록에 나타나지 않으니 초대 코드로 알려 주세요.
        </p>
      {/if}
      {#if status.upnp}
        {#if status.upnp.external}
          <p class="small">
            UPnP 포트 열림: <span class="mono">{status.upnp.external}</span>
            {#if status.upnp.doubleNat}
              <br /><span class="warn-text">외부 IP가 사설 대역입니다(이중 NAT 또는 CGNAT). 인터넷에서는 접속하지 못할 수 있습니다.</span>
            {/if}
          </p>
        {:else if status.upnp.error}
          <p class="small warn-text">{status.upnp.error}</p>
        {/if}
      {/if}
      <label for="invite">초대 코드</label>
      <div class="copy-row">
        <input id="invite" class="mono" readonly value={status.invite} />
        <button onclick={() => copy(status!.invite, "초대 코드")}>복사</button>
      </div>
      {#if status.inviteWithoutPassword}
        <button class="ghost small" onclick={() => copy(status!.inviteWithoutPassword!, "비밀번호 없는 초대 코드")}>
          비밀번호 없이 복사
        </button>
      {/if}
      <p class="muted small">
        친구가 dongsi의 "서버 추가"에 붙여 넣으면 이 방송을 볼 수 있습니다. 같은 LAN·Tailscale에서는 자동으로
        보입니다.
      </p>
    </section>

    <section class="card pane">
      <h2>화질</h2>
      <div class="row">
        <label>
          해상도·fps
          <select bind:value={livePreset} onchange={onLivePreset}>
            {#each presets as p (p.id)}<option value={p.id}>{p.label}</option>{/each}
          </select>
        </label>
        <label>
          코덱
          <select bind:value={liveCodec} onchange={onLiveCodec}>
            {#each CODECS as c (c)}
              <option value={c} disabled={!codecUsable(c).ok}>
                {codecLabel(c)}{codecUsable(c).ok ? "" : ` (${codecUsable(c).why})`}
              </option>
            {/each}
          </select>
        </label>
        <label>
          비트레이트(kbps)
          <input type="number" min="300" max="100000" step="100" bind:value={liveBitrate} />
        </label>
      </div>
      <button class="primary" onclick={applyLive} disabled={applying}>
        {applying ? "적용하는 중…" : "적용"}
      </button>
      <p class="muted small">방송을 끊지 않고 바꿉니다. 코덱이나 해상도를 바꾸면 잠깐 키프레임을 다시 보냅니다.</p>
    </section>

    <section class="card pane stats-pane">
      <h2>상태</h2>
      <dl class="stats">
        <dt>캡처</dt>
        <dd>{status.sourceName}</dd>
        <dt>출력</dt>
        <dd>
          {status.width}×{status.height} · {codecLabel(status.settings.codec)} · 인코더 {status.video.encoder || "–"}
          {status.video.hardware ? "(하드웨어)" : "(소프트웨어)"}
        </dd>
        <dt>프레임</dt>
        <dd>캡처 {fmt(status.video.captureFps)} / 인코딩 {fmt(status.video.encodeFps)} fps · 버림 {status.video.droppedFrames + status.publish.droppedFrames}</dd>
        <dt>인코딩</dt>
        <dd>평균 {fmt(status.video.avgEncodeMs, 1)} ms · 변환 {fmt(status.video.avgConvertMs, 1)} ms ({status.video.gpuConvert ? "GPU" : "CPU"}) · 캡처→출력 {fmt(status.video.avgLatencyMs, 1)} ms</dd>
        <dt>송출</dt>
        <dd>
          {status.publish.connected ? "연결됨" : "연결 중"} · 비디오 {fmt(status.publish.videoKbps)} + 오디오 {fmt(status.publish.audioKbps)} kbps
          · RTT {fmt(status.publish.rttMs)} ms · 손실 {fmt((status.publish.loss ?? 0) * 100, 1)}%
          · 키프레임 요청 {status.publish.keyframeRequests}
        </dd>
        <dt>오디오</dt>
        <dd>
          {#if status.audioStats}
            {AUDIO_LABELS[status.audio]}{discordNote(status.audio, status.audioStats.discordExcluded)} ·
            {fmt(status.audioStats.levelDbfs)} dBFS
            {#if status.audioStats.lastError}<br /><span class="warn-text">{status.audioStats.lastError}</span>{/if}
          {:else}
            없음
          {/if}
        </dd>
        {#if status.videoError}
          <dt>오류</dt>
          <dd class="error">{status.videoError}</dd>
        {/if}
      </dl>
    </section>
  </div>
{:else}
  <!-- 방송 설정 -->
  <h1>방송 시작</h1>

  <div class="grid">
    <section class="card pane">
      <h2>호스팅 방식</h2>
      <label class="radio">
        <input type="radio" bind:group={hosting} value="lan" />
        <span><strong>로컬 랜</strong><br /><span class="muted small">이 PC가 서버가 됩니다. 같은 LAN·Tailscale의 친구가 볼 수 있습니다.</span></span>
      </label>
      <label class="radio">
        <input type="radio" bind:group={hosting} value="direct" />
        <span><strong>PC 직접 호스팅</strong><br /><span class="muted small">UPnP로 공유기 포트를 열어 인터넷에서도 볼 수 있게 합니다.</span></span>
      </label>
      <label class="radio">
        <input type="radio" bind:group={hosting} value="server" disabled={servers.length === 0} />
        <span>
          <strong>추가한 서버</strong><br />
          <span class="muted small">
            {servers.length === 0 ? "추가한 서버가 없습니다." : "전용 서버에서 방송합니다. 여러 명이 동시에 방송할 수 있습니다."}
          </span>
        </span>
      </label>
      {#if hosting === "server"}
        <select bind:value={serverId} aria-label="서버">
          {#each servers as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
        </select>
      {/if}
    </section>

    <section class="card pane">
      <div class="pane-head">
        <h2>캡처 대상</h2>
        <span class="spacer"></span>
        <button class="ghost small" onclick={loadSources}>새로 고침</button>
      </div>
      <div class="source-list" role="radiogroup" aria-label="캡처 대상">
        {#each sources as s (s.id)}
          <label class="source" class:selected={s.id === sourceId}>
            <input type="radio" name="source" value={s.id} bind:group={sourceId} />
            <span class="source-icon" aria-hidden="true">{s.kind === "monitor" ? "🖥" : s.kind === "test" ? "🧪" : "▢"}</span>
            <span class="source-text">
              <span class="source-name">{s.name}</span>
              <span class="muted small">
                {s.kind === "monitor" ? (s.primary ? "주 화면" : "화면") : s.kind === "test" ? "개발용" : s.processName ?? "창"} · {s.width}×{s.height}
              </span>
            </span>
          </label>
        {:else}
          <p class="muted small">캡처할 수 있는 대상이 없습니다.</p>
        {/each}
      </div>
    </section>

    <section class="card pane">
      <h2>화질</h2>
      <div class="row">
        <label>
          해상도·fps
          <select bind:value={presetId}>
            {#each presets as p (p.id)}<option value={p.id}>{p.label}</option>{/each}
          </select>
        </label>
        <label>
          코덱
          <select bind:value={codec}>
            {#each CODECS as c (c)}
              <option value={c} disabled={!codecUsable(c).ok}>
                {codecLabel(c)}{codecUsable(c).ok ? "" : ` (${codecUsable(c).why})`}
              </option>
            {/each}
          </select>
        </label>
        <label>
          비트레이트(kbps)
          <input type="number" min="300" max="100000" step="100" bind:value={bitrate} oninput={() => (bitrateTouched = true)} />
        </label>
      </div>
      <p class="muted small">
        {#if encoders === null}인코더를 확인하는 중…{:else}{codecUsable(codec).why}{/if}
        {#if selectedSource && preset && preset.width > 0}
          · 출력 최대 {preset.width}×{preset.height} (원본 {selectedSource.width}×{selectedSource.height})
        {/if}
      </p>
    </section>

    <section class="card pane">
      <h2>소리와 비밀번호</h2>
      <label>
        소리
        <select bind:value={audio}>
          {#each Object.entries(AUDIO_LABELS) as [mode, label] (mode)}
            {#if mode !== "testTone" || isDev}<option value={mode}>{label}</option>{/if}
          {/each}
        </select>
      </label>
      <label>
        방송 비밀번호(선택)
        <input type="password" bind:value={password} maxlength="128" autocomplete="off" placeholder="비워 두면 누구나 시청" />
      </label>
    </section>
  </div>

  {#if formError}<p class="error" role="alert">{formError}</p>{/if}
  <div class="start-row">
    {#if isDev}
      <button class="ghost" onclick={startTest} disabled={busy}>개발용: 테스트 패턴으로 바로 시작</button>
    {/if}
    <button class="primary big" onclick={start} disabled={busy || !sourceId}>
      {busy ? "시작하는 중…" : "방송 시작"}
    </button>
  </div>
{/if}

<style>
  .back {
    display: inline-block;
    margin-bottom: 1rem;
    font-size: 0.9rem;
  }

  h1 {
    margin-bottom: 1rem;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(340px, 100%), 1fr));
    gap: 0.9rem;
  }

  .pane {
    padding: 1rem 1.1rem;
    display: grid;
    gap: 0.6rem;
    align-content: start;
  }

  .pane p {
    margin: 0;
  }

  .pane-head {
    display: flex;
    align-items: center;
  }

  .stats-pane {
    grid-column: 1 / -1;
  }

  label {
    display: grid;
    gap: 0.25rem;
    font-size: 0.9rem;
    min-width: 0;
  }

  .radio {
    display: flex;
    gap: 0.6rem;
    align-items: flex-start;
    cursor: pointer;
  }

  .radio input {
    width: auto;
    margin-top: 0.3rem;
  }

  .row {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: 0.6rem;
  }

  .source-list {
    display: grid;
    gap: 0.25rem;
    max-height: 15rem;
    overflow-y: auto;
    padding-right: 0.2rem;
  }

  .source {
    position: relative;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .source:hover {
    background: var(--bg-hover);
  }

  .source.selected {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }

  .source input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }

  .source-icon {
    width: 1.4rem;
    text-align: center;
  }

  .source-text {
    display: grid;
    min-width: 0;
  }

  .source-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .copy-row {
    display: flex;
    gap: 0.4rem;
  }

  .copy-row input {
    flex: 1;
    min-width: 0;
  }

  .start-row {
    margin-top: 1.1rem;
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 0.8rem;
  }

  .big {
    padding: 0.7rem 1.6rem;
    font-size: 1rem;
  }

  .live-head {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    margin-bottom: 1rem;
  }

  .live-head h1 {
    margin: 0;
  }

  .live-dot {
    width: 0.75rem;
    height: 0.75rem;
    border-radius: 50%;
    background: var(--danger);
    box-shadow: 0 0 0 4px color-mix(in srgb, var(--danger) 25%, transparent);
  }

  .stats {
    margin: 0;
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.3rem 1rem;
    font-size: 0.88rem;
  }

  .stats dt {
    color: var(--text-muted);
  }

  .stats dd {
    margin: 0;
  }

  .warn-text {
    color: var(--warning);
  }

  .button {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.45rem 0.9rem;
    background: var(--bg-elev);
  }
</style>
