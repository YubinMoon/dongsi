<script lang="ts">
  import { muteButtonLabel, volumeIconName, type MuteState } from "$lib/watch";
  import Icon from "../Icon.svelte";

  let {
    volume = $bindable(1),
    mute,
    showStats,
    popout = false,
    hidden = false,
    ontogglemute,
    ontogglestats,
    onpopout,
    onfullscreen,
    onvolumedrag,
  }: {
    volume?: number;
    mute: MuteState;
    showStats: boolean;
    popout?: boolean;
    /** 재생 중 마우스를 움직이지 않아 막대를 숨긴 상태(D-077). "소리 켜기" 알약은 숨기지 않는다. */
    hidden?: boolean;
    ontogglemute: () => void;
    ontogglestats: () => void;
    onpopout: () => void;
    onfullscreen: () => void;
    /** 음량 바를 끄기 시작하면 true, 놓으면 false(그동안 막대를 숨기지 않는다) */
    onvolumedrag?: (dragging: boolean) => void;
  } = $props();

  /** 음량 바를 누르면 놓을 때까지(바 밖에서 놓아도) 끄는 중이다. */
  function startVolumeDrag() {
    onvolumedrag?.(true);
    const end = () => {
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      onvolumedrag?.(false);
    };
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
  }
</script>

{#if mute.needsUnmute}
  <!-- 자동 재생 정책으로 소리가 막힌 채 재생 중일 때 -->
  <button type="button" class="unmute" onclick={ontogglemute}>
    <Icon name="volume-x" size={17} />소리 켜기
  </button>
{/if}

<div class="w-panel bar w-overlay" class:w-hidden={hidden}>
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
    <input
      type="range"
      min="0"
      max="1"
      step="0.01"
      bind:value={volume}
      disabled={mute.muted}
      onpointerdown={startVolumeDrag}
    />
  </label>
  <span class="spacer"></span>
  <button
    type="button"
    class="w-btn icon"
    aria-label="통계"
    title="통계"
    aria-pressed={showStats}
    onclick={ontogglestats}
  >
    <Icon name="stats" size={18} />
  </button>
  {#if !popout}
    <button type="button" class="w-btn icon" aria-label="새 창으로 보기" title="새 창으로 보기" onclick={onpopout}>
      <Icon name="external" size={18} />
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
