<script lang="ts">
  import { onMount } from "svelte";
  import BroadcastTile from "../BroadcastTile.svelte";
  import Icon from "../Icon.svelte";
  import { api } from "$lib/api";
  import { localTiles } from "$lib/home";
  import type { LocalServerView } from "$lib/types";

  /** 마운트 때, 그리고 5초마다 다시 찾는다. 화면을 떠나면 멈춘다. */
  const SCAN_INTERVAL_MS = 5000;

  let local = $state<LocalServerView[] | null>(null);
  let scanning = $state(false);

  const tiles = $derived(localTiles(local));

  async function scan() {
    if (scanning) return;
    scanning = true;
    try {
      local = await api.scanLocal();
    } catch {
      // 실패하면 이전 결과를 그대로 둔다.
      local = local ?? [];
    } finally {
      scanning = false;
    }
  }

  onMount(() => {
    void scan();
    const timer = setInterval(scan, SCAN_INTERVAL_MS);
    return () => clearInterval(timer);
  });
</script>

<section class="local" aria-labelledby="local-title">
  <div class="section-head">
    <h2 id="local-title">같은 네트워크</h2>
    {#if tiles.length > 0}<span class="sub">LAN과 Tailscale에서 자동으로 찾은 방송</span>{/if}
    <span class="spacer"></span>
    {#if tiles.length > 0}
      <button type="button" class="btn sm" disabled={scanning} onclick={scan}>
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
          locked={tile.locked}
          meta={tile.meta}
        />
      {/each}
    </div>
  {:else}
    <div class="grid">
      <span class="placeholder">같은 LAN이나 Tailscale에서 방송이 시작되면 여기에 바로 나타납니다.</span>
      <span class="placeholder" aria-hidden="true"></span>
      <span class="placeholder" aria-hidden="true"></span>
    </div>
  {/if}
</section>

<style>
  .local {
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
