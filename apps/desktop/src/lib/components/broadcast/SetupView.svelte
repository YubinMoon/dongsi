<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../Icon.svelte";
  import Segmented from "../Segmented.svelte";
  import { api, errorMessage } from "$lib/api";
  import {
    appAudioAllowed,
    audioForSource,
    audioModes,
    followNote,
    audioNote,
    AUDIO_LABELS,
    codecOptions,
    DEFAULT_QUALITY,
    encoderLine,
    firstUsableCodec,
    fpsHint,
    hostingHint,
    mergeThumbnails,
    pickSourceId,
    qualityTarget,
    settingsFor,
    setupSummary,
    startError,
    THUMBNAIL_REFRESH_MS,
    thumbnailIds,
    type QualityChoice,
  } from "$lib/broadcast";
  import { normalizeKbps } from "$lib/bitrate";
  import { bitrateZones } from "$lib/bitrateZones";
  import { setBroadcastStatus } from "$lib/stores/broadcast.svelte";
  import { refreshServers, serverStore } from "$lib/stores/servers.svelte";
  import { toast } from "$lib/toast.svelte";
  import type { AudioMode, HostingKind, SourceThumbnail, SourceView, VideoCodec } from "$lib/types";
  import BitrateField from "./BitrateField.svelte";
  import { Capabilities } from "./capabilities.svelte";
  import Notice from "./Notice.svelte";
  import QualityFields from "./QualityFields.svelte";
  import SourcePicker from "./SourcePicker.svelte";

  const isDev = import.meta.env.DEV;
  const uid = $props.id();
  const caps = new Capabilities();

  let sources = $state<SourceView[]>([]);
  let loadingSources = $state(false);
  let thumbnails = $state<Record<string, SourceThumbnail>>({});
  let thumbnailsBusy = false;

  let hosting = $state<HostingKind>("lan");
  let serverId = $state("");
  let sourceId = $state("");
  /** 해상도(상자, 0×0 = 원본)·주사율(0 = 원본)·코덱. 기본 FHD · 60 · H.264(D-074). */
  let quality = $state<QualityChoice>({ ...DEFAULT_QUALITY });
  let bitrate = $state<number | null>(null);
  /** 비트레이트를 직접 고쳤다. 그 전까지는 "적당" 기본값을 따라간다(캡처 대상이 바뀔 때도). */
  let bitrateTouched = $state(false);
  let audio = $state<AudioMode>("systemExceptDiscord");
  /** 창을 골라 "앱 소리"로 바뀌기 전 모드. 화면으로 돌아가면 이것으로 되돌린다. */
  let audioBeforeApp: AudioMode = "systemExceptDiscord";
  let password = $state("");
  let busy = $state(false);
  let formError = $state<string | null>(null);

  const servers = $derived(serverStore.list);
  const selectedSource = $derived(sources.find((s) => s.id === sourceId) ?? null);
  const codecOpts = $derived(codecOptions(caps.encoders, caps.receivable));
  const target = $derived(qualityTarget(quality, selectedSource));
  const zones = $derived(bitrateZones(target));
  const note = $derived(encoderLine(quality.codec, caps.encoders, caps.receivable, target, selectedSource));
  const rateNote = $derived(fpsHint(quality.fps, selectedSource?.refreshRate));
  const hint = $derived(hostingHint(hosting, servers.length));
  const aNote = $derived(audioNote(audio));
  const fNote = $derived(followNote(selectedSource, audio));
  const summary = $derived(
    setupSummary(target, quality.codec, hosting, servers.find((s) => s.id === serverId)?.name ?? null),
  );

  const hostingOptions = $derived([
    { value: "lan", label: "로컬 랜", icon: "lan" as const },
    { value: "direct", label: "PC 직접 호스팅", icon: "globe" as const },
    { value: "server", label: "추가한 서버", icon: "server" as const, disabled: servers.length === 0 },
  ]);

  // 직접 고치기 전까지 비트레이트는 지금 조합의 "적당" 기본값을 따라간다.
  $effect(() => {
    if (!bitrateTouched) bitrate = zones.recommended;
  });

  /** 해상도·주사율·코덱 칩: 비트레이트는 새 조합의 기본값으로 돌아간다(D-074). */
  function pick(change: Partial<QualityChoice>) {
    quality = { ...quality, ...change };
    bitrateTouched = false;
  }

  /** 캡처 대상을 고른다. 창이면 소리를 그 앱의 소리로 바꾸고, 화면으로 돌아가면 되돌린다. */
  function selectSource(id: string) {
    sourceId = id;
    if (audio !== "app") audioBeforeApp = audio;
    audio = audioForSource(
      sources.find((s) => s.id === id),
      audio,
      audioBeforeApp,
    );
  }

  async function loadSources() {
    loadingSources = true;
    try {
      sources = await api.listCaptureSources();
      const next = pickSourceId(sources, sourceId);
      if (next !== sourceId) selectSource(next);
      void loadThumbnails();
    } catch (e) {
      formError = errorMessage(e);
    } finally {
      loadingSources = false;
    }
  }

  /** 캡처 대상 미리 보기를 찍는다. 실패해도 타일은 아이콘으로 남으므로 알리지 않는다. */
  async function loadThumbnails() {
    const ids = thumbnailIds(sources);
    if (thumbnailsBusy || ids.length === 0 || document.hidden) return;
    thumbnailsBusy = true;
    try {
      thumbnails = mergeThumbnails(thumbnails, await api.sourceThumbnails(ids), ids);
    } catch {
      // 미리 보기 없이도 고를 수 있다.
    } finally {
      thumbnailsBusy = false;
    }
  }

  // 화면이 떠 있는 동안 몇 초마다 미리 보기를 새로 찍는다.
  $effect(() => {
    const timer = setInterval(() => void loadThumbnails(), THUMBNAIL_REFRESH_MS);
    return () => clearInterval(timer);
  });

  async function start() {
    formError = startError(sourceId, hosting, serverId);
    if (formError) return;
    busy = true;
    try {
      const status = await api.startBroadcast({
        hosting,
        serverId: hosting === "server" ? serverId : null,
        source: sourceId,
        settings: settingsFor(quality, normalizeKbps(bitrate, zones.recommended)),
        audio,
        password: password || null,
      });
      setBroadcastStatus(status);
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
    selectSource(test.id);
    audio = "testTone";
    void start();
  }

  onMount(async () => {
    caps.readReceivable();
    const query = new URLSearchParams(location.hash.split("?")[1] ?? "");
    await refreshServers();
    const qs = query.get("server");
    if (qs && servers.some((s) => s.id === qs)) {
      hosting = "server";
      serverId = qs;
    } else if (servers.length > 0) {
      serverId = servers[0].id;
    }
    void loadSources();
    void caps.loadEncoders().then(() => {
      const usable = firstUsableCodec(quality.codec, caps.encoders, caps.receivable);
      if (usable !== quality.codec) pick({ codec: usable });
    });
  });
</script>

<div class="setup">
  <div class="page stack" style="--gap: 20px">
    <div class="intro">
      <h1 class="page-title">방송 시작</h1>
      <p>공유할 화면이나 창을 고르고, 어디서 어떻게 보낼지 정합니다.</p>
    </div>

    <div class="cols">
      <div class="left">
        <SourcePicker
          {sources}
          {thumbnails}
          value={sourceId}
          onselect={selectSource}
          onrefresh={loadSources}
          refreshing={loadingSources}
        />
        {#if fNote}
          <Notice tone="info" icon="refresh" size={12}>{fNote}</Notice>
        {/if}
      </div>

      <section class="settings" aria-label="방송 설정">
        <div class="group">
          <span class="label">호스팅 방식</span>
          <Segmented
            label="호스팅 방식"
            options={hostingOptions}
            value={hosting}
            onchange={(v) => (hosting = v as HostingKind)}
          />
          <p class="hint">{hint}</p>
          {#if hosting === "server" && servers.length > 0}
            <select aria-label="서버" bind:value={serverId}>
              {#each servers as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
            </select>
          {/if}
        </div>

        <div class="sep" aria-hidden="true"></div>

        <QualityFields
          width={quality.width}
          height={quality.height}
          fps={quality.fps}
          codec={quality.codec}
          {codecOpts}
          fpsNote={rateNote}
          onresolution={(width, height) => pick({ width, height })}
          onfps={(fps) => pick({ fps })}
          oncodec={(c) => pick({ codec: c as VideoCodec })}
          {note}
        />

        <BitrateField
          id="{uid}-bitrate"
          bind:value={bitrate}
          {zones}
          {hosting}
          ontouch={() => (bitrateTouched = true)}
        />

        <div class="pair">
          <div class="group grow-md">
            <label class="label" for="{uid}-password">비밀번호 <span class="opt">(선택)</span></label>
            <input
              id="{uid}-password"
              type="password"
              maxlength="128"
              autocomplete="off"
              placeholder="비워 두면 누구나 시청"
              bind:value={password}
            />
          </div>
        </div>

        <div class="sep" aria-hidden="true"></div>

        <div class="group">
          <label class="label" for="{uid}-audio">소리</label>
          <select id="{uid}-audio" bind:value={audio}>
            {#each audioModes(isDev) as mode (mode)}
              <option value={mode} disabled={mode === "app" && !appAudioAllowed(selectedSource)}>
                {AUDIO_LABELS[mode]}
              </option>
            {/each}
          </select>
          <Notice tone={aNote.tone} icon={aNote.icon} size={12}>{aNote.text}</Notice>
        </div>
      </section>
    </div>
  </div>

  <footer class="bar">
    <div class="summary">
      <p class="line">
        <strong>{selectedSource?.name ?? "캡처 대상 없음"}</strong>
        <span class="num">{summary}</span>
      </p>
      {#if formError}<p class="error" role="alert">{formError}</p>{/if}
    </div>
    {#if isDev}
      <button type="button" class="btn ghost" onclick={startTest} disabled={busy}>
        개발용: 테스트 패턴으로 바로 시작
      </button>
    {/if}
    <button type="button" class="btn danger lg start" onclick={start} disabled={busy}>
      <Icon name="broadcast" size={18} />{busy ? "시작하는 중…" : "방송 시작"}
    </button>
  </footer>
</div>

<style>
  .setup {
    display: flex;
    flex-direction: column;
    /* 위쪽 바(56px + 선 1px)를 뺀 높이. 내용이 짧아도 아래 막대는 창 맨 아래에 붙는다. */
    min-height: calc(100vh - 57px);
  }

  .setup > .page {
    flex: 1;
    padding-bottom: 24px;
  }

  .intro {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .intro p {
    font-size: 14px;
    color: var(--text-2);
  }

  .cols {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    gap: 24px;
  }

  .left {
    flex: 1.4 1 480px;
    min-width: 0;
  }

  .settings {
    flex: 1 1 360px;
    min-width: 0;
    max-width: 460px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 20px;
    border-radius: var(--r-panel);
    background: var(--s1);
    box-shadow: inset 0 0 0 1px var(--line);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .sep {
    height: 1px;
    background: var(--line);
  }

  .pair {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }

  .grow-md {
    flex: 1 1 160px;
  }

  .opt {
    font-weight: 400;
    color: var(--text-3);
  }

  .bar {
    position: sticky;
    bottom: 0;
    z-index: 5;
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
    padding: 14px 28px;
    border-top: 1px solid var(--line);
    background: var(--s1);
  }

  .summary {
    flex: 1 1 280px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .line {
    display: flex;
    align-items: baseline;
    gap: 10px;
    flex-wrap: wrap;
  }

  .line strong {
    font-size: 14px;
    font-weight: 650;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .line span {
    font-size: 13px;
    color: var(--text-2);
  }

  .error {
    font-size: 13px;
    color: var(--red-text);
  }

  .start {
    gap: 9px;
    border-radius: var(--r-box);
    font-weight: 700;
  }
</style>
