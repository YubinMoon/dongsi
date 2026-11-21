<script lang="ts">
  import { tick } from "svelte";
  import { page } from "$app/state";
  import { formatClock, secondsSince } from "$lib/format";
  import { onAir } from "$lib/stores/broadcast.svelte";
  import { conn, connect, disconnect } from "$lib/stores/connections.svelte";
  import { serverById, serverStore } from "$lib/stores/servers.svelte";
  import { openAddServer } from "$lib/stores/ui.svelte";
  import { computeTabs, serverMenu, type ServerAction, type ServerStatus } from "$lib/tabs";
  import type { ServerView } from "$lib/types";
  import Icon from "../Icon.svelte";
  import Menu, { type MenuItem } from "../Menu.svelte";
  import RemoveServerDialog from "../RemoveServerDialog.svelte";
  import RenameServerDialog from "../RenameServerDialog.svelte";

  const routeId = $derived(page.route.id);
  /** 지금 보는 서버 화면의 서버 ID */
  const activeId = $derived(routeId === "/server/[id]" ? (page.params.id ?? null) : null);
  // 탭 = 저장한 서버 전부(D-073). 연결 안 된 서버도 흐리게 남고 상태를 점·아이콘으로 보인다.
  const tabs = $derived(computeTabs(serverStore.list, conn, activeId));

  let nav: HTMLElement | undefined = $state();
  /** 탭마다의 ⋯ 메뉴(오른쪽 클릭으로 열 때 쓴다) */
  const menus: Record<string, ReturnType<typeof Menu> | undefined> = {};
  let renameOpen = $state(false);
  let removeOpen = $state(false);
  let target = $state<ServerView | null>(null);

  function run(action: ServerAction, id: string) {
    const server = serverById(id);
    if (!server) return;
    switch (action) {
      case "disconnect":
        void disconnect(id);
        break;
      case "reconnect":
        void connect(id);
        break;
      case "rename":
        target = server;
        renameOpen = true;
        break;
      case "remove":
        target = server;
        removeOpen = true;
        break;
    }
  }

  function menuItems(id: string, status: ServerStatus): MenuItem[] {
    return serverMenu(status).map((e) => ({ ...e, onselect: () => run(e.action, id) }));
  }

  /** 탭을 오른쪽 클릭하면 그 자리에 탭 메뉴를 연다. */
  function onContextMenu(e: MouseEvent, id: string) {
    e.preventDefault();
    menus[id]?.openAt(e.clientX, e.clientY);
  }

  /** 지금 보는 서버를 삭제했으면 홈으로 간다. */
  function onRemoved() {
    if (target && activeId === target.id) location.hash = "#/";
  }

  // 탭이 많아 가로로 넘치면 지금 보는 서버의 탭이 보이게 스크롤한다.
  $effect(() => {
    const id = activeId;
    if (!id || !nav) return;
    void tick().then(() => {
      nav?.querySelector(`[data-server-id="${CSS.escape(id)}"]`)?.scrollIntoView({ block: "nearest", inline: "nearest" });
    });
  });
  const onHome = $derived(routeId === "/");
  const onBroadcast = $derived(routeId === "/broadcast");
  const onSettings = $derived(routeId?.startsWith("/settings") === true);
  const clock = $derived(onAir.status ? formatClock(secondsSince(onAir.status.startedAt, onAir.now)) : "");
</script>

<header>
  <a class="brand" href="#/">
    <span class="logo" aria-hidden="true"><Icon name="play" size={11} /></span>
    dongsi
  </a>

  <nav aria-label="서버" bind:this={nav}>
    <a class="tab home" class:active={onHome} href="#/" aria-current={onHome ? "page" : undefined}>홈</a>
    {#each tabs as tab (tab.id)}
      {@const kind = tab.status.kind}
      <span
        class="tab server"
        class:active={tab.active}
        class:dim={kind !== "connected"}
        data-server-id={tab.id}
        data-status={kind}
      >
        <a
          class="name"
          href="#/server/{tab.id}"
          aria-current={tab.active ? "page" : undefined}
          title={tab.tooltip ?? undefined}
          oncontextmenu={(e) => onContextMenu(e, tab.id)}
        >
          {#if kind === "connecting"}
            <span class="spin" aria-hidden="true"></span>
          {:else if kind === "fingerprint"}
            <span class="warn" aria-hidden="true"><Icon name="alert" size={13} strokeWidth={2} /></span>
          {:else if kind === "failed"}
            <span class="state-dot red" aria-hidden="true"></span>
          {:else if kind === "offline" || kind === "closed"}
            <span class="state-dot gray" aria-hidden="true"></span>
          {/if}
          <span class="label">{tab.name}</span>
          {#if tab.statusText}<span class="sr-only">({tab.statusText})</span>{/if}
        </a>
        <Menu
          bind:this={menus[tab.id]}
          label="'{tab.name}' 서버 메뉴"
          size="xs"
          align="start"
          items={menuItems(tab.id, tab.status)}
        />
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
      <Icon name="gear" size={17} />
    </a>
  </div>
</header>

<RenameServerDialog bind:open={renameOpen} server={target} />
<RemoveServerDialog bind:open={removeOpen} server={target} ondone={onRemoved} />

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

  .tab.server {
    padding-right: 4px;
  }

  .tab.server .name {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 100%;
    max-width: 220px;
    padding: 0 4px 0 12px;
    border-radius: var(--r-btn);
    white-space: nowrap;
    font-weight: inherit;
  }

  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* 연결 안 된 서버(D-073): 이름을 흐리게. 상태 점·아이콘은 흐리게 하지 않는다. */
  .tab.dim .label {
    opacity: 0.55;
  }

  .tab.dim.active .label {
    opacity: 0.75;
  }

  .state-dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }

  .state-dot.gray {
    background: var(--text-3);
  }

  .state-dot.red {
    background: var(--red-dot-2);
  }

  .warn {
    flex: none;
    display: inline-grid;
    color: var(--amber);
  }

  /* 연결하는 중: 작은 원형 회전(Spinner.svelte와 같은 모양을 탭 크기로) */
  .spin {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 1.5px solid var(--line-input);
    border-top-color: var(--text-2);
    animation: tab-spin 0.9s linear infinite;
  }

  @keyframes tab-spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none;
    }
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
