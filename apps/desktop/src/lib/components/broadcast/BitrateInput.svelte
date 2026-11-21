<script lang="ts">
  import { MAX_BITRATE_MBPS, MIN_BITRATE_MBPS, kbpsToMbps, mbpsToKbps } from "$lib/bitrate";

  // 값(value)은 kbps 그대로 두고, 입력란에서만 Mbps(소수 둘째 자리까지)로 보이고 받는다.
  let {
    id,
    value = $bindable(),
    oninput,
    oncommit,
  }: {
    id: string;
    /** kbps. 빈 칸이면 null. */
    value?: number | null;
    oninput?: () => void;
    /** 값을 확정했다: 포커스가 빠지거나(change) Enter를 눌렀을 때. */
    oncommit?: () => void;
  } = $props();
</script>

<span class="field">
  <input
    {id}
    type="number"
    inputmode="decimal"
    min={MIN_BITRATE_MBPS}
    max={MAX_BITRATE_MBPS}
    step="0.01"
    bind:value={() => kbpsToMbps(value), (v) => (value = mbpsToKbps(v))}
    {oninput}
    onchange={() => oncommit?.()}
    onkeydown={(e) => {
      if (e.key === "Enter") oncommit?.();
    }}
  />
  <span class="unit" aria-hidden="true">Mbps</span>
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
    padding: 0 0 0 10px;
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
    padding: 0 10px 0 4px;
    font-size: 12px;
    color: var(--text-3);
  }
</style>
