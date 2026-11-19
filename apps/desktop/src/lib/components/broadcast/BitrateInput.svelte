<script lang="ts">
  let {
    id,
    value = $bindable(),
    oninput,
  }: {
    id: string;
    value?: number | null;
    oninput?: () => void;
  } = $props();
</script>

<span class="field">
  <input {id} type="number" min="300" max="100000" step="100" bind:value {oninput} />
  <span class="unit" aria-hidden="true">kbps</span>
</span>

<style>
  .field {
    display: flex;
    align-items: center;
    height: 36px;
    border: 1px solid var(--line-input);
    border-radius: var(--r-btn);
    background: var(--bg);
    overflow: hidden;
    transition: border-color 0.12s, box-shadow 0.12s;
  }

  .field:focus-within {
    border-color: rgba(255, 255, 255, 0.3);
    box-shadow: 0 0 0 3px var(--line-strong);
  }

  input {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0 0 0 12px;
    border: 0;
    border-radius: 0;
    background: transparent;
    font-variant-numeric: tabular-nums;
  }

  input:focus-visible {
    border: 0;
    box-shadow: none;
  }

  .unit {
    flex: none;
    padding: 0 12px;
    font-size: 12px;
    color: var(--text-3);
  }
</style>
