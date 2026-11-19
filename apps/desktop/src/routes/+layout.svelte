<script lang="ts">
  import "pretendard/dist/web/variable/pretendardvariable.css";
  import "../app.css";
  import { onMount, untrack } from "svelte";
  import { page } from "$app/state";
  import AddServerDialog from "$lib/components/AddServerDialog.svelte";
  import TopBar from "$lib/components/shell/TopBar.svelte";
  import Toasts from "$lib/components/Toasts.svelte";
  import { initBroadcastState, refreshBroadcastStatus } from "$lib/stores/broadcast.svelte";
  import { initConnections, setViewing, syncConnected } from "$lib/stores/connections.svelte";
  import { refreshServers } from "$lib/stores/servers.svelte";
  import { ui } from "$lib/stores/ui.svelte";

  let { children } = $props();

  /** 새 창으로 연 시청 화면: 이 창은 다른 화면으로 가지 않는다. */
  const popout = typeof location !== "undefined" && location.hash.includes("popout=1");

  const routeId = $derived(page.route.id);
  /** 시청 화면과 분리 창은 위쪽 바 없이 창을 꽉 채운다. */
  const full = $derived(popout || routeId?.startsWith("/watch") === true);

  onMount(() => {
    void initConnections();
    void initBroadcastState();
    void refreshServers();
  });

  // 화면을 옮길 때마다: 백엔드의 연결 목록·방송 상태와 맞춘다.
  $effect(() => {
    void page.url.href;
    untrack(() => {
      void syncConnected();
      void refreshBroadcastStatus();
    });
  });

  // 서버 화면이 아닌 곳에서는 "보는 서버"가 없다(서버 화면도 마운트·해제 때 직접 부른다).
  $effect(() => {
    const viewing = page.route.id === "/server/[id]" ? (page.params.id ?? null) : null;
    untrack(() => setViewing(viewing));
  });
</script>

<div class="shell" class:full>
  {#if !full}
    <TopBar />
  {/if}
  <main>
    {@render children()}
  </main>
</div>

<AddServerDialog bind:open={ui.addServerOpen} />
<Toasts />

<style>
  .shell {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
  }

  main {
    flex: 1;
    min-width: 0;
  }

  /* 위쪽 바가 없는 화면: 여백 없이 창을 채운다. */
  .shell.full main {
    display: flex;
    flex-direction: column;
    min-height: 100vh;
  }
</style>
