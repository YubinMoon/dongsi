<script lang="ts">
  import { dismiss, toasts } from "$lib/toast.svelte";
</script>

<div class="toasts" role="status" aria-live="polite">
  {#each toasts as t (t.id)}
    <button class="toast {t.kind}" onclick={() => dismiss(t.id)} title="닫기">
      {t.text}
    </button>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    display: grid;
    gap: 0.5rem;
    max-width: min(420px, calc(100vw - 2rem));
    z-index: 10;
  }

  .toast {
    text-align: left;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-left: 4px solid var(--accent);
    border-radius: var(--radius-sm);
    padding: 0.65rem 0.9rem;
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.35);
  }

  .toast.success {
    border-left-color: var(--success);
  }

  .toast.error {
    border-left-color: var(--danger);
  }
</style>
