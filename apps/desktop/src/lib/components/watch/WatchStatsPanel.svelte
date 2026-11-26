<script lang="ts">
  import type { StatRow, StatSection } from "$lib/watch";
  import type { WatchHealth } from "$lib/watchStatsWindow";
  import Icon from "../Icon.svelte";
  import Sparkline from "./Sparkline.svelte";

  let {
    health = null,
    compactRows = [],
    sections = [],
    detailed = false,
    loading = false,
    ontogglemode,
    oncopy,
    onclose,
  }: {
    health?: WatchHealth | null;
    compactRows?: StatRow[];
    sections?: StatSection[];
    detailed?: boolean;
    loading?: boolean;
    ontogglemode: () => void;
    oncopy: () => void;
    onclose: () => void;
  } = $props();
</script>

{#snippet rowList(rows: StatRow[])}
  <dl>
    {#each rows as row (row.key ?? row.label)}
      <dt>{row.label}</dt>
      <dd
        data-stat={row.key}
        class:ok={row.tone === "ok"}
        class:warn={row.tone === "warn"}
        class:err={row.tone === "err"}
        class:muted={row.tone === "muted"}
      >
        <span class="val">{row.value}</span>
        {#if row.spark}
          <Sparkline
            values={row.spark}
            width={56}
            height={16}
            tone={row.tone === "ok" || row.tone === "warn" || row.tone === "err" ? row.tone : "default"}
            label={row.sparkLabel}
          />
        {/if}
      </dd>
    {/each}
  </dl>
{/snippet}

<!-- 통계 패널: 오른쪽 위 막대 아래. 기본은 요약 모드, 헤더 단추로 상세 전환·AI 진단 복사·닫기. -->
<section class="w-panel stats" class:detailed aria-label="통계">
  <header class="head">
    {#if loading || !health}
      <span class="badge"><span class="dot muted-dot" aria-hidden="true"></span>측정 중</span>
    {:else}
      <span class="badge {health.tone}" data-health={health.tone}>
        <span class="dot {health.tone}-dot" aria-hidden="true"></span>{health.label}
      </span>
    {/if}
    <div class="actions">
      <button
        type="button"
        class="action-btn text"
        aria-expanded={detailed}
        onclick={ontogglemode}
      >
        {detailed ? "요약" : "상세"}
      </button>
      <button
        type="button"
        class="action-btn icon"
        aria-label="진단 통계 복사"
        title="AI 진단용 텍스트 복사"
        disabled={loading}
        onclick={oncopy}
      >
        <Icon name="copy" size={14} />
      </button>
      <button
        type="button"
        class="action-btn icon"
        aria-label="통계 닫기"
        title="통계 닫기"
        onclick={onclose}
      >
        <Icon name="x" size={14} />
      </button>
    </div>
  </header>

  {#if loading}
    <p class="loading">통계를 불러오는 중…</p>
  {:else if !detailed}
    <div class="body">
      {@render rowList(compactRows)}
    </div>
  {:else}
    <div class="body sections">
      {#each sections as section (section.id)}
        <div class="sec" data-section={section.id}>
          <h3>{section.title}</h3>
          {@render rowList(section.rows)}
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .stats {
    position: absolute;
    top: 66px;
    right: 12px;
    z-index: 2;
    width: 288px;
    max-width: calc(100% - 24px);
    padding: 12px 14px;
    background: rgba(14, 15, 18, 0.84);
    -webkit-backdrop-filter: blur(14px);
    backdrop-filter: blur(14px);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1);
    max-height: calc(100% - 66px - 96px);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .stats.detailed {
    width: 344px;
  }

  .head {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding-bottom: 8px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }

  .badge {
    gap: 6px;
    height: 22px;
    font-size: 11px;
  }

  .dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }

  .ok-dot {
    background: var(--green);
  }

  .warn-dot {
    background: var(--amber);
  }

  .err-dot {
    background: var(--red-dot-2);
  }

  .muted-dot {
    background: var(--text-3);
  }

  .actions {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .action-btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 24px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-2);
    font-size: 11px;
    font-weight: 600;
    line-height: 1;
    transition: background-color 0.12s, color 0.12s;
  }

  .action-btn.text {
    padding: 0 8px;
  }

  .action-btn.icon {
    width: 24px;
  }

  .action-btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.16);
    color: #ffffff;
  }

  .action-btn:disabled {
    opacity: 0.45;
  }

  .body {
    overflow-y: auto;
    min-height: 0;
  }

  .sections {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .sec {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .sec + .sec {
    padding-top: 10px;
    border-top: 1px solid rgba(255, 255, 255, 0.07);
  }

  h3 {
    font-size: 11px;
    font-weight: 650;
    letter-spacing: 0.02em;
    color: var(--text-2);
  }

  dl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 6px 12px;
    margin: 0;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  dt {
    color: var(--text-2);
    white-space: nowrap;
  }

  dd {
    margin: 0;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    text-align: right;
    font-family: var(--font-mono);
    color: #ffffff;
    min-width: 0;
  }

  .val {
    overflow-wrap: anywhere;
  }

  dd.ok {
    color: var(--green-text);
  }

  dd.warn {
    color: var(--amber-text);
  }

  dd.err {
    color: var(--red-text);
  }

  dd.muted {
    color: var(--text-2);
  }

  .loading {
    font-size: 12px;
    color: var(--text-2);
    padding: 4px 0;
  }
</style>
