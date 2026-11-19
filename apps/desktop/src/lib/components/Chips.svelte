<script lang="ts">
  export interface ChipOption {
    value: string;
    label: string;
    disabled?: boolean;
    /** 못 쓰는 이유. 예: "재생 불가", "인코더 없음" */
    reason?: string;
  }

  let {
    options,
    value = $bindable(),
    label,
    onchange,
  }: {
    options: ChipOption[];
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

<div class="chips" role="radiogroup" aria-label={label}>
  {#each options as o (o.value)}
    <label class="chip" class:on={value === o.value} class:disabled={o.disabled}>
      <input
        type="radio"
        {name}
        value={o.value}
        checked={value === o.value}
        disabled={o.disabled}
        onchange={() => pick(o.value)}
      />
      {#if o.disabled}
        <s>{o.label}</s>
        {#if o.reason}<span class="reason">{o.reason}</span>{/if}
      {:else}
        {o.label}
      {/if}
    </label>
  {/each}
</div>

<style>
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .chip {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 11px;
    border-radius: var(--r-chip);
    background: rgba(255, 255, 255, 0.06);
    color: var(--text);
    font-size: 12.5px;
    font-weight: 550;
    font-variant-numeric: tabular-nums;
    cursor: pointer;
    transition: background-color 0.12s;
  }

  .chip:hover:not(.on):not(.disabled) {
    background: rgba(255, 255, 255, 0.1);
  }

  .chip.on {
    background: var(--text);
    color: var(--bg);
    font-weight: 650;
  }

  .chip.disabled {
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--line);
    color: var(--text-3);
    cursor: not-allowed;
  }

  .chip:has(input:focus-visible) {
    outline: var(--focus-ring);
    outline-offset: 2px;
  }

  input {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: 0;
    padding: 0;
    opacity: 0;
  }

  s {
    text-decoration-thickness: 1px;
  }

  .reason {
    font-size: 11px;
    font-weight: 500;
  }
</style>
