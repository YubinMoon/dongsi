<script lang="ts">
  import Panel from "./Panel.svelte";
  import {
    audioModeRow,
    discordRow,
    levelRatio,
    levelText,
    sendRows,
    videoRows,
    type StatRow,
  } from "$lib/broadcast";
  import type { BroadcastStatus } from "$lib/types";

  let { status, style }: { status: BroadcastStatus; style?: string } = $props();

  const video = $derived(videoRows(status));
  const send = $derived(sendRows(status.publish));
  const audio = $derived(status.audioStats);
  const audioRows = $derived.by((): StatRow[] => {
    if (!audio) return [{ label: "방식", value: "없음" }];
    const rows: StatRow[] = [audioModeRow(status.audio)];
    const d = discordRow(status.audio, audio);
    if (d) rows.push(d);
    if (audio.lastError) rows.push({ label: "오류", value: audio.lastError, tone: "warn" });
    return rows;
  });
</script>

{#snippet column(title: string, rows: StatRow[], level?: boolean)}
  <div class="col">
    <h3>{title}</h3>
    <dl>
      {#each rows as r (r.label)}
        <div class="row">
          <dt>{r.label}</dt>
          <dd class:ok={r.tone === "ok"} class:warn={r.tone === "warn"} class:muted={r.tone === "muted"}>{r.value}</dd>
        </div>
      {/each}
      {#if level && audio}
        <div class="row">
          <dt>음량</dt>
          <dd>
            <span class="meter-row">
              <span class="meter" aria-hidden="true"><span style:width="{levelRatio(audio.levelDbfs) * 100}%"></span></span>
              {levelText(audio.levelDbfs)}
            </span>
          </dd>
        </div>
      {/if}
    </dl>
  </div>
{/snippet}

<Panel title="상태" {style}>
  {#snippet aside()}
    {#if status.publish.connected}
      <span class="badge ok"><span class="dot ok-dot" aria-hidden="true"></span>연결됨</span>
    {:else}
      <span class="badge warn"><span class="dot warn-dot" aria-hidden="true"></span>연결 중</span>
    {/if}
  {/snippet}

  <div class="cols">
    {@render column("영상", video)}
    {@render column("송출", send)}
    {@render column("소리", audioRows, true)}
  </div>

  {#if status.videoError}
    <p class="video-error" role="alert"><span class="sr-only">영상 오류: </span>{status.videoError}</p>
  {/if}
</Panel>

<style>
  .cols {
    display: flex;
    flex-wrap: wrap;
    gap: 16px 20px;
  }

  .col {
    flex: 1 1 140px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  h3 {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
  }

  dl {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
  }

  .row {
    display: flex;
    flex-direction: column;
  }

  dt {
    color: var(--text-3);
  }

  dd {
    margin: 0;
    color: var(--text);
    font-variant-numeric: tabular-nums;
    overflow-wrap: anywhere;
  }

  dd.ok {
    color: var(--green-text);
  }

  dd.warn {
    color: var(--amber-text);
  }

  dd.muted {
    color: var(--text-2);
  }

  .dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }

  .badge {
    gap: 6px;
    height: 22px;
    font-size: 12px;
  }

  .ok-dot {
    background: var(--green);
  }

  .warn-dot {
    background: var(--amber);
  }

  .meter-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .meter {
    flex: 1;
    max-width: 90px;
    height: 6px;
    border-radius: 3px;
    background: var(--fill-active);
    overflow: hidden;
  }

  .meter span {
    display: block;
    height: 100%;
    border-radius: 3px;
    background: var(--green);
  }

  .video-error {
    font-size: 13px;
    color: var(--red-text);
  }
</style>
