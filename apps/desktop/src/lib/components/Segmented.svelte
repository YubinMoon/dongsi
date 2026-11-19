<script lang="ts">
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  export interface SegmentOption {
    value: string;
    label: string;
    icon?: IconName;
    disabled?: boolean;
  }

  let {
    options,
    value = $bindable(),
    label,
    onchange,
  }: {
    options: SegmentOption[];
    value?: string;
    /** 그룹의 aria-label */
    label: string;
    onchange?: (value: string) => void;
  } = $props();

  const name = $props.id();

  function pick(v: string) {
    value = v;
    onchange?.(v);
  }
</script>

<div class="segmented" role="radiogroup" aria-label={label}>
  {#each options as o (o.value)}
    <label class="seg" class:on={value === o.value} class:disabled={o.disabled}>
      <input
        type="radio"
        {name}
        value={o.value}
        checked={value === o.value}
        disabled={o.disabled}
        onchange={() => pick(o.value)}
      />
      {#if o.icon}<Icon name={o.icon} size={15} />{/if}
      {o.label}
    </label>
  {/each}
</div>

<style>
  .segmented {
    display: flex;
    gap: 2px;
    padding: 3px;
    border-radius: 9px;
    background: var(--bg);
    box-shadow: inset 0 0 0 1px var(--line);
  }

  .seg {
    position: relative;
    flex: 1 1 0;
    min-width: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    height: 34px;
    padding: 0 8px;
    border-radius: var(--r-chip);
    color: var(--text-2);
    font-size: 12.5px;
    font-weight: 550;
    white-space: nowrap;
    cursor: pointer;
    transition: background-color 0.12s, color 0.12s;
  }

  .seg:hover:not(.on):not(.disabled) {
    background: rgba(255, 255, 255, 0.06);
    color: var(--text);
  }

  .seg.on {
    background: var(--text);
    color: var(--bg);
    font-weight: 650;
  }

  .seg.disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .seg:has(input:focus-visible) {
    outline: var(--focus-ring);
    outline-offset: 1px;
  }

  input {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: 0;
    padding: 0;
    opacity: 0;
  }
</style>
