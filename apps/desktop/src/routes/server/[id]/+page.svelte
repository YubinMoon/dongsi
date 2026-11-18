<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { page } from "$app/state";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { api, codecLabel, errorMessage, qualityLabel } from "$lib/api";
  import type { KeyEvent, ServerClosedEvent, ServerLive, ServerView } from "$lib/types";
  import LockIcon from "$lib/components/LockIcon.svelte";

  const serverId = $derived(page.params.id ?? "");

  let server = $state<ServerView | null>(null);
  let live = $state<ServerLive | null>(null);
  let error = $state<string | null>(null);
  let connecting = $state(false);
  let unlisten: UnlistenFn[] = [];

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
      connecting = live === null;
      live = await api.connectServer(id);
      error = null;
      // 첫 접속이면 지문이 저장되었으므로 서버 정보를 다시 읽는다.
      if (!found.fingerprint) {
        server = (await api.listServers()).find((s) => s.id === id) ?? found;
      }
    } catch (e) {
      error = errorMessage(e);
      live = null;
    } finally {
      connecting = false;
    }
  }

  $effect(() => {
    load(serverId);
  });

  onMount(async () => {
    unlisten.push(
      await listen<KeyEvent>("server-changed", (e) => {
        if (e.payload.key === serverId) load(serverId);
      }),
      await listen<ServerClosedEvent>("server-closed", (e) => {
        if (e.payload.key === serverId) {
          live = null;
          error = `서버와 연결이 끊어졌습니다: ${e.payload.reason}`;
        }
      }),
    );
  });

  onDestroy(() => {
    for (const u of unlisten) u();
  });
</script>

<a class="back muted" href="#/">← 서버 목록</a>

{#if server}
  <div class="head">
    <div>
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
    <span class="spacer"></span>
    {#if live?.canPublish}
      <a class="button primary" href="#/broadcast?server={serverId}">이 서버에서 방송하기</a>
    {/if}
  </div>
{/if}

{#if error}
  <div class="card notice-error">
    <p class="error" role="alert">{error}</p>
    {#if server}<button onclick={() => load(serverId)}>다시 연결</button>{/if}
  </div>
{:else if connecting}
  <p class="muted">서버에 연결하는 중…</p>
{:else if live}
  <div class="section-head list-head">
    <h2>방송 목록</h2>
    <span class="muted small">{live.serverName}</span>
  </div>

  {#if live.broadcasts.length > 0}
    <ul class="broadcast-grid">
      {#each live.broadcasts as broadcast (broadcast.broadcasterId)}
        <li>
          <a class="card broadcast" href="#/watch/{serverId}/{broadcast.broadcasterId}">
            <span class="thumb" aria-hidden="true">{broadcast.broadcasterName.slice(0, 1)}</span>
            <span class="meta">
              <span class="name">
                {broadcast.broadcasterName}
                {#if broadcast.locked}<LockIcon />{/if}
                {#if broadcast.mine}<span class="badge">내 방송</span>{/if}
              </span>
              <span class="muted small">
                {qualityLabel(broadcast.height, broadcast.fps)} · {codecLabel(broadcast.codec)}
                · 시청자 {broadcast.viewers}명
              </span>
              {#if broadcast.locked && broadcast.passwordSaved}
                <span class="badge ok">비밀번호 기억됨</span>
              {/if}
            </span>
          </a>
        </li>
      {/each}
    </ul>
  {:else}
    <div class="card empty">
      <p>지금 진행 중인 방송이 없습니다.</p>
    </div>
  {/if}
{/if}



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
    display: flex;
    align-items: flex-start;
    gap: 1rem;
  }

  .notice-error {
    padding: 1rem 1.2rem;
    display: grid;
    gap: 0.6rem;
    justify-items: start;
  }

  .notice-error p {
    margin: 0;
  }

  .button {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.45rem 0.9rem;
    background: var(--bg-elev);
    white-space: nowrap;
  }

  .button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
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
    border: 1px solid var(--border);
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
