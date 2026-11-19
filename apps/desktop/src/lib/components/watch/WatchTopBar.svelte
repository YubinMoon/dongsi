<script lang="ts">
  import Icon from "../Icon.svelte";

  let {
    popout = false,
    backHref,
    name,
    live = false,
    serverName = "",
    infoText = null,
    viewers = null,
    onclose,
  }: {
    /** 새 창으로 연 시청 화면이면 "돌아가기" 대신 "창 닫기" */
    popout?: boolean;
    backHref: string;
    name: string;
    /** LIVE 배지 */
    live?: boolean;
    serverName?: string;
    /** 오른쪽 막대의 "H.264 · 1920×1080 · 60fps". null이면 오른쪽 막대를 숨긴다. */
    infoText?: string | null;
    viewers?: number | null;
    onclose: () => void;
  } = $props();
</script>

<div class="bars">
  <div class="w-panel left">
    {#if popout}
      <button type="button" class="w-btn" onclick={onclose}><Icon name="x" size={17} /><span>창 닫기</span></button>
    {:else}
      <a class="w-btn" href={backHref}><Icon name="arrow-left" size={17} /><span>돌아가기</span></a>
    {/if}
    <span class="sep" aria-hidden="true"></span>
    <div class="who">
      <h1>{name}</h1>
      {#if live}<span class="badge live">LIVE</span>{/if}
      {#if serverName}<span class="server">{serverName}</span>{/if}
    </div>
  </div>
  <span class="spacer"></span>
  {#if infoText !== null}
    <div class="w-panel right">
      <span>{infoText}</span>
      {#if viewers !== null}
        <span class="viewers"><Icon name="eye" size={15} />시청자 {viewers}명</span>
      {/if}
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
    gap: 14px;
    height: 42px;
    padding: 0 14px;
    font-size: 13px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }

  .viewers {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: #ffffff;
  }
</style>
