<script lang="ts">
  import { tick, type Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  let {
    open = $bindable(false),
    title,
    width = 440,
    children,
    actions,
  }: {
    open?: boolean;
    title: string;
    width?: number;
    children: Snippet;
    actions?: Snippet;
  } = $props();

  let dialog: HTMLDialogElement | undefined = $state();
  const uid = $props.id();
  const titleId = `${uid}-title`;

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) {
      dialog.showModal();
      void focusFirst();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  /** 입력란이 있으면 거기로, 없으면 아래 단추 줄의 첫 단추(확인 대화 상자는 "취소")로 초점을 옮긴다. */
  async function focusFirst() {
    await tick();
    const field = dialog?.querySelector<HTMLElement>(
      "[data-autofocus], .body input:not([type=hidden]):not([disabled]), .body textarea, .body select",
    );
    const target = field ?? dialog?.querySelector<HTMLElement>(".actions button:not([disabled])");
    target?.focus();
  }
</script>

<!-- Esc로 닫으면 close 이벤트가 오므로 open도 함께 내린다. -->
<dialog
  bind:this={dialog}
  aria-labelledby={titleId}
  style:--dialog-width="{width}px"
  onclose={() => (open = false)}
>
  {#if open}
    <div class="head">
      <h2 id={titleId}>{title}</h2>
      <button type="button" class="icon-btn sm" aria-label="닫기" onclick={() => (open = false)}>
        <Icon name="x" />
      </button>
    </div>
    <div class="body">
      {@render children()}
    </div>
    {#if actions}
      <div class="actions">
        {@render actions()}
      </div>
    {/if}
  {/if}
</dialog>

<style>
  dialog {
    width: min(var(--dialog-width), calc(100vw - 32px));
    max-height: calc(100vh - 32px);
    margin: auto;
    padding: 20px 22px 22px;
    overflow: auto;
    border: 0;
    border-radius: var(--r-panel);
    background: var(--s1);
    color: var(--text);
    box-shadow: var(--shadow-dialog);
  }

  dialog[open] {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  dialog::backdrop {
    background: rgba(5, 6, 8, 0.68);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  h2 {
    flex: 1;
    margin: 0;
    font-size: 17px;
    font-weight: 650;
    letter-spacing: -0.015em;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
</style>
