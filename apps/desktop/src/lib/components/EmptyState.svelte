<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  let {
    icon,
    title,
    description,
    tone = "neutral",
    actions,
  }: {
    icon: IconName;
    title: string;
    description?: string;
    tone?: "neutral" | "danger";
    actions?: Snippet;
  } = $props();
</script>

<div class="empty" role={tone === "danger" ? "alert" : undefined}>
  <div class="inner">
    <span class="icon" class:danger={tone === "danger"} aria-hidden="true"><Icon name={icon} size={22} /></span>
    <p class="title">{title}</p>
    {#if description}<p class="desc">{description}</p>{/if}
    {#if actions}
      <div class="actions">{@render actions()}</div>
    {/if}
  </div>
</div>

<style>
  .empty {
    display: grid;
    place-items: center;
    min-height: 300px;
    padding: 32px;
    border-radius: var(--r-empty);
    border: 1px dashed var(--line-strong);
  }

  .inner {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    max-width: 440px;
    text-align: center;
  }

  .icon {
    width: 52px;
    height: 52px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgba(255, 255, 255, 0.08);
    color: #ffffff;
  }

  .icon.danger {
    background: rgba(229, 56, 59, 0.16);
    color: var(--red-text);
  }

  .title {
    font-size: 16px;
    font-weight: 600;
    letter-spacing: -0.01em;
    text-wrap: pretty;
  }

  .desc {
    margin-top: -6px;
    font-size: 13px;
    color: var(--text-2);
    text-wrap: pretty;
  }

  .actions {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
</style>
