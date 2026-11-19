<script lang="ts">
  import { dismiss, toasts } from "$lib/toast.svelte";
  import Icon from "./Icon.svelte";
</script>

<!-- 아래 가운데. 누르면 닫힌다. -->
<div class="toasts" role="status" aria-live="polite">
  {#each toasts as t (t.id)}
    <button type="button" class="toast {t.kind}" class:plain={t.kind === "info"} title="닫기" onclick={() => dismiss(t.id)}>
      {#if t.kind === "success"}
        <span class="mark" aria-hidden="true"><Icon name="check" size={14} /></span>
      {:else if t.kind === "error"}
        <span class="mark" aria-hidden="true"><Icon name="x" size={14} /></span>
      {/if}
      {t.text}
    </button>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 24px;
    z-index: 60;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 0 24px;
    pointer-events: none;
  }

  .toast {
    pointer-events: auto;
    display: inline-flex;
    align-items: center;
    gap: 10px;
    max-width: min(560px, 100%);
    min-height: 42px;
    padding: 8px 16px 8px 10px;
    border-radius: var(--r-box);
    background: var(--s3);
    box-shadow: var(--shadow-pop);
    color: var(--text);
    font-size: 13px;
    font-weight: 550;
    text-align: left;
  }

  .toast.plain {
    padding-left: 16px;
  }

  .mark {
    flex: none;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    display: grid;
    place-items: center;
  }

  .success .mark {
    background: rgba(76, 195, 138, 0.18);
    color: var(--green-text);
  }

  .error .mark {
    background: rgba(229, 56, 59, 0.18);
    color: var(--red-text);
  }
</style>
