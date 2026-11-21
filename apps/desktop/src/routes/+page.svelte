<script lang="ts">
  import FirstRunBox from "$lib/components/home/FirstRunBox.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import LiveBroadcasts from "$lib/components/home/LiveBroadcasts.svelte";
  import RemoveServerDialog from "$lib/components/RemoveServerDialog.svelte";
  import RenameServerDialog from "$lib/components/RenameServerDialog.svelte";
  import ServerCard from "$lib/components/home/ServerCard.svelte";
  import { countLabel } from "$lib/home";
  import { dismissCard } from "$lib/stores/connections.svelte";
  import { refreshServers, serverStore } from "$lib/stores/servers.svelte";
  import { openAddServer } from "$lib/stores/ui.svelte";
  import type { ServerView } from "$lib/types";

  let renameOpen = $state(false);
  let renameTarget = $state<ServerView | null>(null);
  let removeOpen = $state(false);
  let removeTarget = $state<ServerView | null>(null);

  const servers = $derived(serverStore.list);
  /** 목록을 읽었고 비어 있으면 처음 실행 상자를 맨 위에 보인다. */
  const firstRun = $derived(serverStore.loaded && serverStore.error === null && servers.length === 0);

  function startRename(server: ServerView) {
    renameTarget = server;
    renameOpen = true;
  }

  function startRemove(server: ServerView) {
    removeTarget = server;
    removeOpen = true;
  }
</script>

<div class="page stack" class:first-run={firstRun}>
  {#if firstRun}
    <FirstRunBox />
  {/if}

  <LiveBroadcasts />

  {#if !firstRun}
    <section class="servers" aria-labelledby="servers-title">
      <div class="section-head">
        <h2 id="servers-title">내 서버</h2>
        {#if serverStore.loaded && serverStore.error === null}
          <span class="sub">{countLabel(servers.length)}</span>
        {/if}
        <span class="spacer"></span>
        <button type="button" class="btn sm" onclick={openAddServer}>
          <Icon name="plus" size={14} />서버 추가
        </button>
      </div>

      {#if serverStore.error}
        <div class="load-error" role="alert">
          <span>{serverStore.error}</span>
          <button type="button" class="btn sm" onclick={refreshServers}>
            <Icon name="refresh" size={13} />다시 불러오기
          </button>
        </div>
      {:else if !serverStore.loaded}
        <p class="loading">불러오는 중…</p>
      {/if}

      {#if servers.length > 0}
        <div class="grid">
          {#each servers as server (server.id)}
            <ServerCard {server} onrename={startRename} onremove={startRemove} />
          {/each}
        </div>
      {/if}
    </section>
  {/if}
</div>

<RenameServerDialog bind:open={renameOpen} server={renameTarget} />
<RemoveServerDialog
  bind:open={removeOpen}
  server={removeTarget}
  ondone={() => removeTarget && dismissCard(removeTarget.id)}
/>

<style>
  .page {
    --gap: 32px;
  }

  /* b-first-run은 위 여백 32, 구획 간격 36 */
  .page.first-run {
    --gap: 36px;
    padding-top: 32px;
  }

  .servers {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 16px;
  }

  .loading {
    font-size: 13px;
    color: var(--text-3);
  }

  .load-error {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    font-size: 13px;
    color: var(--red-text);
  }
</style>
