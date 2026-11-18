<script lang="ts">
  import "../app.css";
  import { page } from "$app/state";
  import Toasts from "$lib/components/Toasts.svelte";

  let { children } = $props();

  const onServers = $derived(page.route.id === "/" || page.route.id?.startsWith("/server") === true);
  const onSettings = $derived(page.route.id === "/settings");
</script>

<div class="shell">
  <header>
    <a class="brand" href="#/">dongsi</a>
    <nav aria-label="주 메뉴">
      <a href="#/" class:active={onServers} aria-current={onServers ? "page" : undefined}>서버</a>
      <a href="#/settings" class:active={onSettings} aria-current={onSettings ? "page" : undefined}>설정</a>
    </nav>
  </header>
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
