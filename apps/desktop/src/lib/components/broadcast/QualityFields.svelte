<script lang="ts">
  import Chips from "../Chips.svelte";
  import type { PresetView } from "$lib/types";

  type CodecOption = { value: string; label: string; disabled: boolean; reason?: string };

  let {
    presets,
    presetId,
    codec,
    codecOpts,
    onpreset,
    oncodec,
    note = null,
  }: {
    presets: PresetView[];
    presetId: string;
    codec: string;
    codecOpts: CodecOption[];
    onpreset: (id: string) => void;
    oncodec: (codec: string) => void;
    /** 코덱 칩 아래 한 줄(인코더·출력 크기) */
    note?: string | null;
  } = $props();

  const options = $derived(presets.map((p) => ({ value: p.id, label: p.label })));
</script>

<div class="group">
  <span class="label">해상도·fps</span>
  <Chips {options} value={presetId} label="해상도·fps" onchange={onpreset} />
</div>

<div class="group">
  <span class="label">코덱</span>
  <Chips options={codecOpts} value={codec} label="코덱" onchange={oncodec} />
  {#if note}<p class="hint num">{note}</p>{/if}
</div>

<style>
  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
</style>
