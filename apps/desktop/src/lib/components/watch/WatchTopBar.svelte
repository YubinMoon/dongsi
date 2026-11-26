<script lang="ts">
  import { viewersLabel } from "$lib/watch";
  import Icon from "../Icon.svelte";

  let {
    popout = false,
    backHref,
    name,
    live = false,
    serverName = "",
    viewers = null,
    hidden = false,
    onclose,
  }: {
    /** 새 창으로 연 시청 화면이면 "나가기" 대신 "창 닫기" */
    popout?: boolean;
    backHref: string;
    name: string;
    /** LIVE 배지 */
    live?: boolean;
    serverName?: string;
    /** 오른쪽 막대의 시청자 수(눈 아이콘 + 숫자). null이면 오른쪽 막대를 숨긴다. */
    viewers?: number | null;
    /** 재생 중 마우스를 움직이지 않아 막대를 숨긴 상태(D-077) */
    hidden?: boolean;
    onclose: () => void;
  } = $props();

  /** 지금 보이는 "나가기" 링크 또는 "창 닫기" 단추 */
  let leaveControl: HTMLElement | undefined = $state();

  /** 그 단추를 누른 것과 같게 나간다(ESC, src/lib/watchKeys.ts). */
  export function leave() {
    leaveControl?.click();
  }
</script>

<div class="bars w-overlay" class:w-hidden={hidden}>
  <div class="w-panel left">
    {#if popout}
      <button bind:this={leaveControl} type="button" class="w-btn danger" onclick={onclose}><Icon name="x" size={17} /><span>창 닫기</span></button>
    {:else}
      <a bind:this={leaveControl} class="w-btn danger" href={backHref}><Icon name="exit" size={17} /><span>나가기</span></a>
    {/if}
    <span class="sep" aria-hidden="true"></span>
    <div class="who">
      <h1>{name}</h1>
      {#if live}<span class="badge live">LIVE</span>{/if}
      {#if serverName}<span class="server">{serverName}</span>{/if}
    </div>
  </div>
  <span class="spacer"></span>
  {#if viewers !== null}
    <div class="w-panel right" role="img" aria-label={viewersLabel(viewers)} title={viewersLabel(viewers)}>
      <Icon name="eye" size={16} /><span>{viewers}</span>
    </div>
  {/if}
</div>

<style>
  .bars {
    position: absolute;
    top: 12px;
    left: 12px;
    right: 12px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    pointer-events: none;
  }

  .bars > :global(*) {
    pointer-events: auto;
  }

  /* 숨긴 막대는 누를 수 없다(watch.css .w-hidden). 위의 auto보다 앞서게 여기서 다시 끈다. */
  .bars.w-hidden > :global(*) {
    pointer-events: none;
  }

  .spacer {
    flex: 1;
  }

  .left {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px;
    min-width: 0;
  }

  .sep {
    flex: none;
    width: 1px;
    height: 20px;
    margin-left: 4px;
    background: var(--line-strong);
  }

  .who {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 0 10px 0 8px;
  }

  h1 {
    font-size: 15px;
    font-weight: 650;
    letter-spacing: -0.01em;
    white-space: nowrap;
  }

  .server {
    font-size: 13px;
    color: var(--text-2);
    white-space: nowrap;
  }

  .right {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 42px;
    padding: 0 14px;
    font-size: 13px;
    font-weight: 600;
    color: #ffffff;
    font-variant-numeric: tabular-nums;
  }
</style>
