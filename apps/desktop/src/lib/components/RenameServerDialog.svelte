<script lang="ts">
  import { untrack } from "svelte";
  import { api } from "$lib/api";
  import { errorMessage } from "$lib/errors";
  import { refreshServers } from "$lib/stores/servers.svelte";
  import type { ServerView } from "$lib/types";
  import Dialog from "./Dialog.svelte";

  let {
    open = $bindable(false),
    server,
    ondone,
  }: {
    open?: boolean;
    server: ServerView | null;
    ondone?: () => void;
  } = $props();

  const formId = $props.id();
  let name = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);

  // 열릴 때마다 지금 이름으로 채운다(열려 있는 동안 목록이 갱신돼도 입력을 지우지 않는다).
  $effect(() => {
    if (open) {
      untrack(() => {
        name = server?.name ?? "";
        error = null;
        busy = false;
      });
    }
  });

  const canSave = $derived(!busy && server !== null && name.trim() !== "");

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!canSave || !server) return;
    busy = true;
    error = null;
    try {
      await api.renameServer(server.id, name.trim());
      await refreshServers();
      open = false;
      ondone?.();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<Dialog bind:open title="서버 이름 변경" width={420}>
  <form id={formId} class="form" onsubmit={save}>
    <label class="label" for="{formId}-name">새 이름</label>
    <input id="{formId}-name" class="lg" bind:value={name} maxlength="64" autocomplete="off" spellcheck="false" />
    {#if server}
      <span class="hint">
        내 목록에서만 바뀝니다. 주소: <span class="mono">{server.addrs[0] ?? ""}</span>
      </span>
    {/if}
    {#if error}<p class="err" role="alert">{error}</p>{/if}
  </form>

  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (open = false)}>취소</button>
    <button type="submit" form={formId} class="btn primary" disabled={!canSave}>저장</button>
  {/snippet}
</Dialog>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
  }

  .err {
    font-size: 13px;
    color: var(--red-text);
  }
</style>
