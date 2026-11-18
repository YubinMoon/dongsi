<script lang="ts">
  import { page } from "$app/state";
  import { api, errorMessage, qualityLabel } from "$lib/api";
  import { toast } from "$lib/toast.svelte";
  import type { BroadcastView, ServerBroadcasts, ServerView } from "$lib/types";
  import LockIcon from "$lib/components/LockIcon.svelte";
  import PasswordDialog from "$lib/components/PasswordDialog.svelte";

  const serverId = $derived(page.params.id ?? "");

  let server = $state<ServerView | null>(null);
  let data = $state<ServerBroadcasts | null>(null);
  let error = $state<string | null>(null);

  let passwordOpen = $state(false);
  let passwordTarget = $state<BroadcastView | null>(null);

  async function load(id: string) {
    try {
      const servers = await api.listServers();
      const found = servers.find((s) => s.id === id) ?? null;
      if (!found) {
        server = null;
        error = "서버를 찾을 수 없습니다. 목록에서 삭제되었을 수 있습니다.";
        return;
      }
      server = found;
      data = await api.listBroadcasts(id);
      error = null;
    } catch (e) {
      error = errorMessage(e);
    }
  }

  $effect(() => {
    load(serverId);
  });

  function open(broadcast: BroadcastView) {
    if (broadcast.locked && !broadcast.passwordSaved) {
      passwordTarget = broadcast;
      passwordOpen = true;
      return;
    }
    toast("시청 기능은 서버 연결을 구현한 뒤에 동작합니다");
  }

  async function savePassword(password: string) {
    if (!passwordTarget) return;
    await api.rememberPassword(serverId, passwordTarget.broadcasterId, password);
    toast("비밀번호를 기억했습니다", "success");
    await load(serverId);
  }
</script>

<a class="back muted" href="#/">← 서버 목록</a>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if server}
  <div class="head">
    <h1>{server.name}</h1>
    <p class="muted small">
      <span class="mono">{server.addrs.join(", ")}</span>
      {#if server.fingerprint}
        · 지문 <span class="mono" title={server.fingerprint.full}>{server.fingerprint.short}…</span>
      {:else}
        · 첫 접속 전(지문 미확인)
      {/if}
    </p>
  </div>

  {#if data?.sample}
    <p class="notice">
      서버 연결은 아직 구현하지 않았습니다. 아래 방송 목록은 화면 확인용 예시입니다.
    </p>
  {/if}

  <div class="section-head list-head">
    <h2>방송 목록</h2>
  </div>

  {#if data && data.broadcasts.length > 0}
    <ul class="broadcast-grid">
      {#each data.broadcasts as broadcast (broadcast.broadcasterId)}
        <li>
          <button class="card broadcast" onclick={() => open(broadcast)}>
            <span class="thumb" aria-hidden="true">{broadcast.broadcasterName.slice(0, 1)}</span>
            <span class="meta">
              <span class="name">
                {broadcast.broadcasterName}
                {#if broadcast.locked}<LockIcon />{/if}
              </span>
              <span class="muted small">{qualityLabel(broadcast.height, broadcast.fps)}</span>
              {#if broadcast.locked && broadcast.passwordSaved}
                <span class="badge ok">비밀번호 기억됨</span>
              {/if}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  {:else if data}
    <div class="card empty">
      <p>지금 진행 중인 방송이 없습니다.</p>
    </div>
  {:else}
    <p class="muted">불러오는 중…</p>
  {/if}
{/if}

<PasswordDialog
  bind:open={passwordOpen}
  broadcasterName={passwordTarget?.broadcasterName ?? ""}
  onsubmit={savePassword}
/>

<style>
  .back {
    display: inline-block;
    margin-bottom: 1rem;
    font-size: 0.9rem;
  }

  .back:hover {
    color: var(--text);
  }

  .head {
    margin-bottom: 1rem;
  }

  .head p {
    margin: 0.3rem 0 0;
  }

  .list-head {
    margin-top: 1.4rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .broadcast-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 0.8rem;
  }

  .broadcast {
    width: 100%;
    padding: 0;
    overflow: hidden;
    text-align: left;
    display: grid;
  }

  .thumb {
    aspect-ratio: 16 / 9;
    display: grid;
    place-items: center;
    font-size: 2rem;
    font-weight: 700;
    color: var(--text-muted);
    background: linear-gradient(135deg, color-mix(in srgb, var(--accent) 22%, var(--bg)), var(--bg));
    border-bottom: 1px solid var(--border);
  }

  .meta {
    display: grid;
    gap: 0.2rem;
    padding: 0.7rem 0.9rem;
    justify-items: start;
  }

  .name {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    font-weight: 600;
  }

  .empty {
    padding: 1.4rem;
  }

  .empty p {
    margin: 0;
  }
</style>
