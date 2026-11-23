<script lang="ts">
  import { untrack } from "svelte";
  import Dialog from "../Dialog.svelte";
  import Notice from "./Notice.svelte";
  import SourcePicker from "./SourcePicker.svelte";
  import { api, errorMessage } from "$lib/api";
  import { mergeThumbnails, switchableSources, switchNotes, THUMBNAIL_REFRESH_MS, thumbnailIds } from "$lib/broadcast";
  import { withJosa } from "$lib/josa";
  import { setBroadcastStatus } from "$lib/stores/broadcast.svelte";
  import { toast } from "$lib/toast.svelte";
  import type { BroadcastStatus, SourceThumbnail, SourceView } from "$lib/types";

  let { open = $bindable(false), status }: { open?: boolean; status: BroadcastStatus } = $props();

  let sources = $state<SourceView[]>([]);
  let loading = $state(false);
  let thumbnails = $state<Record<string, SourceThumbnail>>({});
  let thumbnailsBusy = false;
  /** 고른 대상. 열 때 지금 대상으로 시작한다. */
  let chosen = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  const selected = $derived(sources.find((s) => s.id === chosen) ?? null);
  /** 소리가 없는 방송(오디오가 시작되지 않음)이면 null: 소리는 그대로다. */
  const notes = $derived(switchNotes(selected, status.audioStats ? status.audio : null));
  /** 지금 보내는 대상(닫혀 기다리는 중이면 같은 대상도 다시 고를 수 있다). */
  const current = $derived(chosen === status.sourceId && !status.sourceWaiting);

  // 열 때마다 지금 대상을 고른 채로 목록을 새로 읽고, 열려 있는 동안 몇 초마다 미리 보기를 새로 찍는다.
  // 상태는 1초마다 새로 오므로 열고 닫을 때만 다시 돌게 한다.
  $effect(() => {
    if (!open) return;
    untrack(() => {
      chosen = status.sourceId;
      error = null;
      void loadSources();
    });
    const timer = setInterval(() => void loadThumbnails(), THUMBNAIL_REFRESH_MS);
    return () => clearInterval(timer);
  });

  async function loadSources() {
    loading = true;
    try {
      sources = switchableSources(await api.listCaptureSources());
      void loadThumbnails();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      loading = false;
    }
  }

  /** 미리 보기를 찍는다. 실패해도 타일은 아이콘으로 남으므로 알리지 않는다. */
  async function loadThumbnails() {
    const ids = thumbnailIds(sources);
    if (!open || thumbnailsBusy || ids.length === 0 || document.hidden) return;
    thumbnailsBusy = true;
    try {
      thumbnails = mergeThumbnails(thumbnails, await api.sourceThumbnails(ids), ids);
    } catch {
      // 미리 보기 없이도 고를 수 있다.
    } finally {
      thumbnailsBusy = false;
    }
  }

  async function apply() {
    if (!chosen || current || busy) return;
    busy = true;
    error = null;
    try {
      const next = await api.switchBroadcastSource(chosen);
      setBroadcastStatus(next);
      toast(`${withJosa(next.sourceName, "(으)로")} 바꿨습니다`, "success");
      open = false;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<!--
  방송 중 화면 전환(D-086): 방송을 끊지 않고 캡처 대상만 바꾼다. 타일을 누르는 것만으로는 바뀌지 않고
  "바꾸기"를 눌러야 바뀐다(잘못 눌러 다른 창이 나가지 않게). 취소·바꾸기는 목록이 길어도 바로 보이게
  아래 단추 줄이 아니라 목록 머리의 "새로 고침" 옆에 둔다.
-->
<Dialog bind:open title="화면 전환" width={820}>
  <SourcePicker
    {sources}
    {thumbnails}
    value={chosen}
    onselect={(id) => (chosen = id)}
    onrefresh={loadSources}
    refreshing={loading}
  >
    {#snippet actions()}
      <button type="button" class="btn sm" data-autofocus onclick={() => (open = false)}>취소</button>
      <button type="button" class="btn sm primary" onclick={apply} disabled={busy || !chosen || current}>
        {busy ? "바꾸는 중…" : "바꾸기"}
      </button>
    {/snippet}
  </SourcePicker>
  {#each notes as n (n)}
    <Notice tone="info" icon="volume" size={12}>{n}</Notice>
  {/each}
  {#if error}<p class="err" role="alert">{error}</p>{/if}
</Dialog>

<style>
  .err {
    font-size: 13px;
    color: var(--red-text);
  }
</style>
