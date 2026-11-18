<script lang="ts">
  import "../app.css";
  import { page } from "$app/state";
  import Toasts from "$lib/components/Toasts.svelte";

  let { children } = $props();

  /** 새 창으로 연 시청 화면: 위쪽 메뉴를 숨긴다(이 창은 다른 화면으로 가지 않는다). */
  const popout = typeof location !== "undefined" && location.hash.includes("popout=1");

  const onServers = $derived(
    page.route.id === "/" ||
      page.route.id?.startsWith("/server") === true ||
      page.route.id?.startsWith("/watch") === true,
  );
  const onSettings = $derived(page.route.id === "/settings");
  const onBroadcast = $derived(page.route.id === "/broadcast");
</script>

<div class="shell" class:popout>
  {#if !popout}
  <header>
    <a class="brand" href="#/">dongsi</a>
    <nav aria-label="주 메뉴">
      <a href="#/" class:active={onServers} aria-current={onServers ? "page" : undefined}>서버</a>
      <a href="#/broadcast" class:active={onBroadcast} aria-current={onBroadcast ? "page" : undefined}>방송</a>
      <a href="#/settings" class:active={onSettings} aria-current={onSettings ? "page" : undefined}>설정</a>
    </nav>
  </header>
  {/if}
  <main>
    {@render children()}
  </main>
</div>

<Toasts />

<style>
  .shell {
    min-height: 100vh;
    display: grid;
    grid-template-rows: auto 1fr;
  }

  .shell.popout {
    grid-template-rows: 1fr;
  }

  .shell.popout main {
    padding: 0.6rem 0.8rem;
    max-width: none;
  }

  header {
    display: flex;
    align-items: center;
    gap: 1.5rem;
    padding: 0.7rem 1.4rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-elev);
  }

  .brand {
    font-weight: 700;
    font-size: 1.1rem;
    letter-spacing: 0.02em;
  }

  nav {
    display: flex;
    gap: 0.3rem;
  }

  nav a {
    padding: 0.3rem 0.75rem;
    border-radius: var(--radius-sm);
    color: var(--text-muted);
  }

  nav a:hover {
    color: var(--text);
    background: var(--bg-hover);
  }

  nav a.active {
    color: var(--text);
    background: var(--bg-hover);
  }

  main {
    width: 100%;
    max-width: 1100px;
    margin: 0 auto;
    padding: 1.4rem;
  }
</style>
