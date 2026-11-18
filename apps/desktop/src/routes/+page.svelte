<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorMessage, qualityLabel } from "$lib/api";
  import { toast } from "$lib/toast.svelte";
  import type { AddResult, LocalBroadcasts, ServerView } from "$lib/types";
  import AddServerDialog from "$lib/components/AddServerDialog.svelte";
  import LockIcon from "$lib/components/LockIcon.svelte";
  import Modal from "$lib/components/Modal.svelte";

  let servers = $state<ServerView[]>([]);
  let local = $state<LocalBroadcasts | null>(null);
  let loading = $state(true);
  let loadError = $state<string | null>(null);

  let addOpen = $state(false);

  let renameOpen = $state(false);
  let renameTarget = $state<ServerView | null>(null);
  let renameValue = $state("");
  let renameError = $state<string | null>(null);

  let removeOpen = $state(false);
  let removeTarget = $state<ServerView | null>(null);

  async function refresh() {
    try {
      [servers, local] = await Promise.all([api.listServers(), api.listLocalBroadcasts()]);
      loadError = null;
    } catch (e) {
      loadError = errorMessage(e);
    } finally {
      loading = false;
    }
  }

  onMount(refresh);

  function onadded(result: AddResult) {
    const name = result.server.name;
    const messages = {
      added: `'${name}' 서버를 추가했습니다`,
      updated: `'${name}' 서버의 정보를 갱신했습니다`,
      alreadyExists: `'${name}'(으)로 이미 저장된 서버입니다`,
    };
    toast(messages[result.outcome], result.outcome === "alreadyExists" ? "info" : "success");
    refresh();
  }

  function startRename(server: ServerView) {
    renameTarget = server;
    renameValue = server.name;
    renameError = null;
    renameOpen = true;
  }

  async function submitRename(event: SubmitEvent) {
    event.preventDefault();
    if (!renameTarget) return;
    try {
      await api.renameServer(renameTarget.id, renameValue);
      renameOpen = false;
      refresh();
    } catch (e) {
      renameError = errorMessage(e);
    }
  }

  function startRemove(server: ServerView) {
    removeTarget = server;
    removeOpen = true;
  }

  async function confirmRemove() {
    if (!removeTarget) return;
    const target = removeTarget;
    try {
      await api.removeServer(target.id);
      removeOpen = false;
      toast(`'${target.name}' 서버를 삭제했습니다`);
      refresh();
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }
</script>

<section>
  <div class="section-head">
    <h1>내 서버</h1>
    <span class="spacer"></span>
    <button class="primary" onclick={() => (addOpen = true)}>+ 서버 추가</button>
  </div>

  {#if loading}
    <p class="muted">불러오는 중…</p>
  {:else if loadError}
    <p class="error" role="alert">{loadError}</p>
  {:else if servers.length === 0}
    <div class="card empty">
      <p>아직 추가한 서버가 없습니다.</p>
      <p class="muted small">서버 주소를 입력하거나, 친구에게 받은 초대 코드를 붙여 넣어 추가하세요.</p>
      <button class="primary" onclick={() => (addOpen = true)}>서버 추가</button>
    </div>
  {:else}
    <ul class="server-grid">
      {#each servers as server (server.id)}
        <li class="card server-card">
          <a class="server-main" href="#/server/{server.id}">
            <span class="server-name">{server.name}</span>
            <span class="mono muted">
              {server.addrs[0]}{#if server.addrs.length > 1}&nbsp;외 {server.addrs.length - 1}개{/if}
            </span>
            <span class="badges">
              {#if server.fingerprint}
                <span class="badge ok" title={server.fingerprint.full}>지문 확인됨</span>
              {:else}
                <span class="badge warn" title="처음 접속할 때 인증서 지문을 확인합니다">첫 접속 전</span>
              {/if}
              {#if server.savedPasswords > 0}
                <span class="badge">비밀번호 {server.savedPasswords}개 기억</span>
              {/if}
            </span>
          </a>
          <div class="server-actions">
            <button class="ghost small" onclick={() => startRename(server)}>이름 변경</button>
            <button class="ghost danger small" onclick={() => startRemove(server)}>삭제</button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<section class="local">
  <div class="section-head">
    <h2>로컬 네트워크</h2>
    {#if local?.sample}<span class="badge warn">예시</span>{/if}
  </div>
  <p class="muted small">
    같은 LAN이나 Tailscale에서 시작된 방송이 여기에 자동으로 나타납니다. 감지 기능을 만들기 전이라
    지금은 예시를 보여 줍니다.
  </p>
  {#if local && local.items.length > 0}
    <ul class="local-list">
      {#each local.items as item (item.addr)}
        <li>
          <button
            class="card local-item"
            onclick={() => toast("로컬 방송 연결은 아직 구현되지 않았습니다")}
          >
            <span class="avatar" aria-hidden="true">{item.hostName.slice(0, 1)}</span>
            <span class="local-text">
              <span class="local-name">
                {item.hostName}
                {#if item.locked}<LockIcon />{/if}
              </span>
              <span class="muted small">
                <span class="mono">{item.addr}</span> · {item.via === "tailscale" ? "Tailscale" : "LAN"} ·
                {qualityLabel(item.height, item.fps)}
              </span>
            </span>
          </button>
        </li>
      {/each}
    </ul>
  {:else if local}
    <p class="muted">같은 네트워크에서 진행 중인 방송이 없습니다.</p>
  {/if}
</section>

<AddServerDialog bind:open={addOpen} {onadded} />

<Modal bind:open={renameOpen} title="서버 이름 변경">
  <form id="rename-server" onsubmit={submitRename}>
    <label for="rename-input">새 이름</label>
    <input id="rename-input" bind:value={renameValue} maxlength="64" />
  </form>
  {#if renameError}<p class="error small" role="alert">{renameError}</p>{/if}
  {#snippet actions()}
    <button type="button" onclick={() => (renameOpen = false)}>취소</button>
    <button type="submit" form="rename-server" class="primary">저장</button>
  {/snippet}
</Modal>

<Modal bind:open={removeOpen} title="서버 삭제">
  <p>'{removeTarget?.name}' 서버를 목록에서 삭제할까요?</p>
  <p class="muted small">이 서버에서 기억한 방송 비밀번호도 함께 지워집니다.</p>
  {#snippet actions()}
    <button type="button" onclick={() => (removeOpen = false)}>취소</button>
    <button type="button" class="danger solid" onclick={confirmRemove}>삭제</button>
  {/snippet}
</Modal>

<style>
  section + section {
    margin-top: 2.2rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .empty {
    padding: 1.6rem;
    display: grid;
    justify-items: start;
    gap: 0.4rem;
  }

  .empty p {
    margin: 0;
  }

  .empty button {
    margin-top: 0.6rem;
  }

  .server-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 0.8rem;
  }

  .server-card {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .server-main {
    display: grid;
    gap: 0.3rem;
    padding: 0.9rem 1rem 0.6rem;
    flex: 1;
  }

  .server-main:hover {
    background: var(--bg-hover);
  }

  .server-name {
    font-weight: 600;
    font-size: 1.02rem;
  }

  .badges {
    display: flex;
    gap: 0.35rem;
    flex-wrap: wrap;
    margin-top: 0.2rem;
  }

  .server-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.2rem;
    padding: 0.3rem 0.5rem;
    border-top: 1px solid var(--border);
  }

  .local > p {
    margin: 0 0 0.8rem;
  }

  .local-list {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 0.6rem;
  }

  .local-item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.8rem;
    padding: 0.7rem 0.9rem;
    text-align: left;
  }

  .local-text {
    display: grid;
    gap: 0.1rem;
  }

  .local-name {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    font-weight: 600;
  }

  .avatar {
    flex: none;
    width: 2.2rem;
    height: 2.2rem;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--accent) 25%, var(--bg-hover));
    font-weight: 700;
  }
</style>
