<script lang="ts">
  import { onMount } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import LiveView from "$lib/components/broadcast/LiveView.svelte";
  import SetupView from "$lib/components/broadcast/SetupView.svelte";
  import { onAir, refreshBroadcastStatus } from "$lib/stores/broadcast.svelte";
  import { toast } from "$lib/toast.svelte";

  let loaded = $state(false);

  /** 방송 중인지(상태 객체가 아니라 참/거짓만 봐서, 1초마다 바뀌어도 타이머를 다시 만들지 않는다). */
  const live = $derived(onAir.status !== null);

  // 방송 중에는 1초마다 상태를 다시 읽는다(통계·시청자 수).
  $effect(() => {
    if (!live) return;
    const timer = setInterval(() => void refreshBroadcastStatus(), 1000);
    return () => clearInterval(timer);
  });

  onMount(() => {
    let unlisten: UnlistenFn | null = null;
    let gone = false;
    void (async () => {
      await refreshBroadcastStatus();
      loaded = true;
      try {
        // 상태를 비우는 일은 저장소가 하고, 이 화면은 알림만 띄운다. 화면이 시작 화면으로 바뀌면서 소스를 다시 읽는다.
        const un = await listen<{ reason: string }>("broadcast-stopped", (e) => {
          toast(`방송이 끝났습니다: ${e.payload.reason}`, "error");
        });
        if (gone) un();
        else unlisten = un;
      } catch (e) {
        console.warn("broadcast-stopped 이벤트를 들을 수 없습니다", e);
      }
    })();
    return () => {
      gone = true;
      unlisten?.();
    };
  });
</script>

{#if onAir.status}
  <LiveView status={onAir.status} />
{:else if loaded}
  <SetupView />
{:else}
  <div class="page">
    <p class="loading">불러오는 중…</p>
  </div>
{/if}

<style>
  .loading {
    color: var(--text-2);
  }
</style>
