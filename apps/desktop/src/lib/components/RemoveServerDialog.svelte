<script lang="ts">
  import { api } from "$lib/api";
  import { errorMessage } from "$lib/errors";
  import { conn, disconnect } from "$lib/stores/connections.svelte";
  import { refreshServers } from "$lib/stores/servers.svelte";
  import { toast } from "$lib/toast.svelte";
  import type { ServerView } from "$lib/types";
  import Dialog from "./Dialog.svelte";
  import Icon from "./Icon.svelte";

  let {
    open = $bindable(false),
    server,
    ondone,
  }: {
    open?: boolean;
    server: ServerView | null;
    ondone?: () => void;
  } = $props();

  let error = $state<string | null>(null);
  let busy = $state(false);

  $effect(() => {
    if (open) {
      error = null;
      busy = false;
    }
  });

  async function remove() {
    if (!server || busy) return;
    const { id, name } = server;
    busy = true;
    error = null;
    try {
      await api.removeServer(id);
      // 연결돼 있던 서버면 탭도 함께 지운다(연결은 remove_server가 이미 닫았다).
      if (conn.tabs.order.includes(id)) await disconnect(id, { localOnly: true });
      await refreshServers();
      toast(`'${name}' 서버를 삭제했습니다`, "success");
      open = false;
      ondone?.();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<Dialog bind:open title="서버 삭제" width={420}>
  {#if server}
    <div class="text">
      <p class="main">'{server.name}' 서버를 목록에서 삭제할까요?</p>
      <p class="sub">이 서버에서 기억한 방송 비밀번호도 함께 지워집니다.</p>
    </div>
  {/if}
  {#if error}<p class="err" role="alert">{error}</p>{/if}

  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (open = false)}>취소</button>
    <button type="button" class="btn danger" disabled={busy || !server} onclick={remove}>
      <Icon name="trash" size={15} />삭제
    </button>
  {/snippet}
</Dialog>

<style>
  .text {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .main {
    font-size: 14px;
    text-wrap: pretty;
  }

  .sub {
    font-size: 13px;
    color: var(--text-2);
    text-wrap: pretty;
  }

  .err {
    font-size: 13px;
    color: var(--red-text);
  }
</style>
