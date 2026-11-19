<script lang="ts">
  import "pretendard/dist/web/variable/pretendardvariable.css";
  import "../app.css";
  import { onMount, untrack } from "svelte";
  import { preloadCode } from "$app/navigation";
  import { page } from "$app/state";
  import AddServerDialog from "$lib/components/AddServerDialog.svelte";
  import TopBar from "$lib/components/shell/TopBar.svelte";
  import Toasts from "$lib/components/Toasts.svelte";
  import { warmUpWebRtc } from "$lib/rtcWarmup";
  import { initBroadcastState, refreshBroadcastStatus } from "$lib/stores/broadcast.svelte";
  import { initConnections, setViewing, syncConnected } from "$lib/stores/connections.svelte";
  import { initServerEvents, refreshServers } from "$lib/stores/servers.svelte";
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
    void initServerEvents();
    void refreshServers();

    // 첫 시청 예열: 첫 RTCPeerConnection 생성이 약 350ms 걸리므로(src/lib/rtcWarmup.ts) 앱이 한가할 때 미리 한 번 만든다.
    // 시청 화면 코드도 미리 불러 둔다(첫 이동 때 모듈을 받아 해석하는 시간. 개발 모드에서는 vite가 처음 변환하느라 1초 넘게 걸렸다).
    // 해시 라우터라 경로를 해시로 준다(`get_url_path`가 url.hash를 본다). 매개변수 값은 아무것이나 된다.
    // 화면 표시를 늦추지 않는 것이 목적이라 시작하고 1.5초 뒤, 브라우저가 한가할 때(requestIdleCallback, 없으면 바로) 한다.
    // 분리 창이거나 시작 경로가 시청 화면이면 그 화면이 바로 PC를 만들 테니 하지 않는다.
    let warmTimer: ReturnType<typeof setTimeout> | undefined;
    if (!popout && !location.hash.startsWith("#/watch")) {
      warmTimer = setTimeout(() => {
        const warm = () => {
          warmUpWebRtc();
          preloadCode("/#/watch/_/_").catch(() => {});
        };
        if (typeof requestIdleCallback === "function") requestIdleCallback(warm);
        else warm();
      }, 1500);
    }
    return () => clearTimeout(warmTimer);
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
