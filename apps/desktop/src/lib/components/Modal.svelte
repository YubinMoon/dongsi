<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    open = $bindable(false),
    title,
    children,
    actions,
  }: {
    open?: boolean;
    title: string;
    children: Snippet;
    actions?: Snippet;
  } = $props();

  let dialog: HTMLDialogElement | undefined = $state();
  const titleId = `modal-${Math.random().toString(36).slice(2)}`;

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    else if (!open && dialog.open) dialog.close();
  });
</script>

<!-- Esc로 닫으면 close 이벤트가 오므로 open도 함께 내린다. -->
<dialog bind:this={dialog} aria-labelledby={titleId} onclose={() => (open = false)}>
  <h2 id={titleId}>{title}</h2>
  <div class="body">
    {@render children()}
  </div>
  {#if actions}
    <div class="actions">
      {@render actions()}
    </div>
  {/if}
</dialog>

<style>
  dialog {
    width: min(520px, calc(100vw - 2rem));
    padding: 1.2rem 1.3rem;
    color: var(--text);
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 18px 50px rgb(0 0 0 / 0.45);
  }

  dialog::backdrop {
    background: rgb(0 0 0 / 0.55);
  }

  .body {
    margin-top: 0.9rem;
    display: grid;
    gap: 0.8rem;
  }

  /* 간격은 grid gap으로 맞추므로 문단 기본 여백은 없앤다. */
  .body > :global(p) {
    margin: 0;
  }

  .actions {
    margin-top: 1.2rem;
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
</style>
