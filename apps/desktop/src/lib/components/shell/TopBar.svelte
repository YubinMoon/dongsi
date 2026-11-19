<script lang="ts">
  import { page } from "$app/state";
  import { formatClock, secondsSince } from "$lib/format";
  import { onAir } from "$lib/stores/broadcast.svelte";
  import { conn, disconnect } from "$lib/stores/connections.svelte";
  import { serverStore } from "$lib/stores/servers.svelte";
  import { openAddServer } from "$lib/stores/ui.svelte";
  import { computeTabs } from "$lib/tabs";
  import Icon from "../Icon.svelte";

  const tabs = $derived(computeTabs(conn.tabs, serverStore.list, conn.viewing));
  const routeId = $derived(page.route.id);
  const onHome = $derived(routeId === "/");
  const onBroadcast = $derived(routeId === "/broadcast");
  const onSettings = $derived(routeId === "/settings");
  const clock = $derived(onAir.status ? formatClock(secondsSince(onAir.status.startedAt, onAir.now)) : "");
</script>

<header>
  <a class="brand" href="#/">
    <span class="logo" aria-hidden="true"><Icon name="play" size={11} /></span>
    dongsi
  </a>

  <nav aria-label="서버">
    <a class="tab home" class:active={onHome} href="#/" aria-current={onHome ? "page" : undefined}>홈</a>
    {#each tabs as tab (tab.id)}
      <span class="tab server" class:active={tab.active} class:closed={tab.closed}>
        <a
          class="name"
          href="#/server/{tab.id}"
          aria-current={tab.active ? "page" : undefined}
          title={tab.closed ? "연결이 끊어졌습니다" : undefined}>{tab.name}</a
        >
        <button
          type="button"
          class="close"
          aria-label="'{tab.name}' 연결 끊기"
          title="연결 끊기"
          onclick={() => disconnect(tab.id)}
        >
          <Icon name="x" size={12} />
        </button>
      </span>
    {/each}
    <button type="button" class="icon-btn sm add" aria-label="서버 추가" onclick={openAddServer}>
      <Icon name="plus" />
    </button>
  </nav>

  <div class="right">
    {#if onAir.status}
      <a class="onair" href="#/broadcast" aria-current={onBroadcast ? "page" : undefined}>
        <span class="dot" aria-hidden="true"></span>방송 중
        <span class="clock mono num">{clock}</span>
      </a>
    {:else}
      <a class="btn primary start" href="#/broadcast" aria-current={onBroadcast ? "page" : undefined}>
        <span class="dot" aria-hidden="true"></span>방송 시작
      </a>
    {/if}
    <a class="icon-btn" href="#/settings" aria-label="설정" aria-current={onSettings ? "page" : undefined}>
      <Icon name="sliders" size={17} />
    </a>
  </div>
</header>

<style>
  header {
    display: flex;
    align-items: center;
    gap: 20px;
    flex-wrap: wrap;
    min-height: 56px;
    padding: 10px 20px;
    border-bottom: 1px solid var(--line);
  }

  .brand {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 9px;
    font-size: 15px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }

  .logo {
    width: 22px;
    height: 22px;
    border-radius: var(--r-badge);
    background: var(--text);
    color: var(--bg);
    display: grid;
    place-items: center;
  }

  nav {
    flex: 1 1 320px;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 4px;
    overflow-x: auto;
    scrollbar-width: thin;
  }

  .tab {
    flex: none;
    display: inline-flex;
    align-items: center;
    height: 32px;
    border-radius: var(--r-btn);
    font-size: 13px;
    font-weight: 500;
    color: var(--text-2);
    transition: background-color 0.12s, color 0.12s;
  }

  .tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text);
  }

  .tab.active {
    background: var(--fill-active);
    color: var(--text);
    font-weight: 600;
  }

  .tab.home {
    gap: 8px;
    padding: 0 12px;
  }

  .tab.server .name {
    display: inline-flex;
    align-items: center;
    height: 100%;
    padding: 0 4px 0 12px;
    border-radius: var(--r-btn);
    white-space: nowrap;
    font-weight: inherit;
  }

  /* 연결이 끊긴 채 남은 탭(그 서버 화면을 보고 있을 때만 남는다): 글자를 흐리게 */
  .tab.closed .name {
    opacity: 0.55;
  }

  .close {
    width: 24px;
    height: 24px;
    margin-right: 4px;
    display: grid;
    place-items: center;
    border-radius: var(--r-badge);
    color: var(--text-3);
  }

  .close:hover {
    background: var(--fill-hover);
    color: var(--text);
  }

  .add {
    flex: none;
  }

  .right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--red-dot);
  }

  .start {
    gap: 8px;
  }

  /* 지금 이 화면이 방송 화면이면 고리로 표시한다(b-broadcast-setup) */
  .start[aria-current="page"] {
    box-shadow:
      0 0 0 2px var(--bg),
      0 0 0 3px rgba(255, 255, 255, 0.5);
  }

  .onair {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 14px;
    border-radius: var(--r-btn);
    background: rgba(229, 56, 59, 0.16);
    box-shadow: inset 0 0 0 1px rgba(229, 56, 59, 0.45);
    color: var(--red-pill-text);
    font-size: 13px;
    font-weight: 650;
    transition: background-color 0.12s;
  }

  .onair:hover {
    background: rgba(229, 56, 59, 0.22);
  }

  .onair .dot {
    background: var(--red-dot-2);
  }

  .clock {
    font-weight: 500;
  }
</style>
