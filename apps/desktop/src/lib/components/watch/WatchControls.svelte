<script lang="ts">
  import { muteButtonLabel, volumeIconName, type MuteState } from "$lib/watch";
  import Icon from "../Icon.svelte";

  let {
    volume = $bindable(1),
    mute,
    showStats,
    popout = false,
    ontogglemute,
    ontogglestats,
    onpopout,
    onfullscreen,
  }: {
    volume?: number;
    mute: MuteState;
    showStats: boolean;
    popout?: boolean;
    ontogglemute: () => void;
    ontogglestats: () => void;
    onpopout: () => void;
    onfullscreen: () => void;
  } = $props();
</script>

{#if mute.needsUnmute}
  <!-- 자동 재생 정책으로 소리가 막힌 채 재생 중일 때 -->
  <button type="button" class="unmute" onclick={ontogglemute}>
    <Icon name="volume-x" size={17} />소리 켜기
  </button>
{/if}

<div class="w-panel bar">
  <button
    type="button"
    class="w-btn icon"
    aria-label={muteButtonLabel(mute)}
    aria-keyshortcuts="M"
    title={muteButtonLabel(mute)}
    onclick={ontogglemute}
  >
    <Icon name={volumeIconName(mute)} size={19} />
  </button>
  <label class="volume">
    <span class="sr-only">음량</span>
    <input type="range" min="0" max="1" step="0.01" bind:value={volume} disabled={mute.muted} />
  </label>
  <span class="spacer"></span>
  <button type="button" class="w-btn" aria-pressed={showStats} onclick={ontogglestats}>
    <Icon name="stats" size={17} /><span>통계</span>
  </button>
  {#if !popout}
    <button type="button" class="w-btn" onclick={onpopout}>
      <Icon name="external" size={17} /><span>새 창으로 보기</span>
    </button>
  {/if}
  <button type="button" class="w-btn" aria-keyshortcuts="F" onclick={onfullscreen}>
    <Icon name="fullscreen" size={17} /><span>전체 화면</span><kbd class="w-kbd" aria-hidden="true">F</kbd>
  </button>
</div>

<style>
  .bar {
    position: absolute;
    left: 12px;
    right: 12px;
    bottom: 12px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding: 6px;
  }

  .spacer {
    flex: 1;
  }

  .volume {
    display: inline-flex;
    align-items: center;
    height: 34px;
    padding: 0 6px;
  }

  .volume input {
    width: 110px;
  }

  .unmute {
    position: absolute;
    left: 50%;
    bottom: 76px;
    z-index: 2;
    transform: translateX(-50%);
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 40px;
    padding: 0 18px;
    border-radius: 999px;
    background: var(--text);
    color: var(--bg);
    font-size: 14px;
    font-weight: 650;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
    white-space: nowrap;
  }

  .unmute:hover {
    background: #ffffff;
  }
</style>
