<script lang="ts">
  import { onMount } from "svelte";
  import BroadcastTile from "../BroadcastTile.svelte";
  import Icon from "../Icon.svelte";
  import { api } from "$lib/api";
  import { liveTiles } from "$lib/home";
  import { conn, connectSoon, isConnected } from "$lib/stores/connections.svelte";
  import { serverStore } from "$lib/stores/servers.svelte";
  import type { LocalServerView, ServerLive } from "$lib/types";

  /**
   * 홈 "방송 중"(D-071): 내가 볼 수 있는 방송을 한 목록으로.
   * - 저장한 서버: 레이아웃이 자동으로 연결해 두고(`startAutoConnect`), 서버가 보낸 방송 목록(`conn.live`)을 쓴다.
   * - 같은 LAN·Tailscale: 마운트 때, 그리고 5초마다 로컬 감지로 찾는다. 화면을 떠나면 멈춘다.
   */
  const SCAN_INTERVAL_MS = 5000;
  /** 방송 시간("30분")을 다시 계산하는 간격 */
  const CLOCK_INTERVAL_MS = 30_000;

  let local = $state<LocalServerView[] | null>(null);
  let scanning = $state(false);
  let now = $state(Date.now());
  /** 지난 감지에서 본 저장한 서버(새로 나타난 서버만 바로 연결해 본다). */
  let seenSaved = new Set<string>();

  /** 연결돼 있는 서버의 상태만(끊긴 서버의 지난 방송 목록은 뺀다). */
  const connectedLive = $derived.by(() => {
    const out: Record<string, ServerLive> = {};
    for (const [key, state] of Object.entries(conn.live)) {
      if (isConnected(key)) out[key] = state;
    }
    return out;
  });
  const tiles = $derived(liveTiles(serverStore.list, connectedLive, local, now));

  async function scan() {
    if (scanning) return;
    scanning = true;
    try {
      local = await api.scanLocal();
      // 저장한 서버가 같은 네트워크에서 새로 보이면(방송을 켠 친구 PC 등) 자동 연결을 기다리지 않고 바로 연결해 본다.
      const saved = new Set(local.filter((s) => s.saved).map((s) => s.key));
      const fresh = [...saved].filter((key) => !seenSaved.has(key));
      seenSaved = saved;
      if (fresh.length > 0) connectSoon(fresh);
    } catch {
      // 실패하면 이전 결과를 그대로 둔다.
      local = local ?? [];
    } finally {
      scanning = false;
    }
  }

  /** 새로 고침: 다시 찾고, 연결 안 된 저장한 서버도 바로 다시 시도한다. */
  function refresh() {
    void scan();
    connectSoon();
  }

  onMount(() => {
    void scan();
    const scanTimer = setInterval(scan, SCAN_INTERVAL_MS);
    const clockTimer = setInterval(() => (now = Date.now()), CLOCK_INTERVAL_MS);
    return () => {
      clearInterval(scanTimer);
      clearInterval(clockTimer);
    };
  });
</script>

<section class="on-air" aria-labelledby="on-air-title">
  <div class="section-head">
    <h2 id="on-air-title">방송 중</h2>
    {#if tiles.length > 0}<span class="sub">내 서버와 같은 네트워크(LAN·Tailscale)의 방송</span>{/if}
    <span class="spacer"></span>
    {#if tiles.length > 0}
      <button type="button" class="btn sm" disabled={scanning} onclick={refresh}>
        <Icon name="refresh" size={14} />새로 고침
      </button>
    {:else}
      <span class="searching"><span class="dot" aria-hidden="true"></span>찾는 중…</span>
    {/if}
  </div>

  {#if tiles.length > 0}
    <div class="grid">
      {#each tiles as tile (tile.key)}
        <BroadcastTile
          seed={tile.seed}
          name={tile.name}
          href={tile.href}
          via={tile.via}
          elapsed={tile.elapsed}
          viewers={tile.viewers}
          locked={tile.locked}
          passwordSaved={tile.locked && tile.passwordSaved}
          meta={tile.meta}
        />
      {/each}
    </div>
  {:else}
    <div class="grid">
      <span class="placeholder">내 서버나 같은 LAN·Tailscale에서 방송이 시작되면 여기에 바로 나타납니다.</span>
      <span class="placeholder" aria-hidden="true"></span>
      <span class="placeholder" aria-hidden="true"></span>
    </div>
  {/if}
</section>

<style>
  .on-air {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 20px;
  }

  .searching {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 13px;
    color: var(--text-2);
  }

  .dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-3);
  }

  .placeholder {
    display: grid;
    place-items: center;
    aspect-ratio: 16 / 9;
    padding: 20px;
    border-radius: var(--r-tile);
    border: 1px dashed var(--line-strong);
    color: var(--text-3);
    font-size: 13px;
    text-align: center;
    text-wrap: pretty;
  }
</style>
