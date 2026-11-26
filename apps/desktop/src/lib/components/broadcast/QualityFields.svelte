<script lang="ts">
  import Chips from "../Chips.svelte";
  import { FRAME_RATES, RESOLUTIONS, resolutionValue } from "$lib/broadcast";

  // 해상도·주사율 칩(D-074). 코덱은 H.264 하나라 고르지 않는다(D-103). 값은 부모가 들고, 칩을 누르면 알린다.
  let {
    width,
    height,
    fps,
    onresolution,
    onfps,
    fpsNote = null,
    note = null,
  }: {
    /** 출력 크기 상한(FHD·QHD 상자, D-097) */
    width: number;
    height: number;
    /** 30·60(D-101) */
    fps: number;
    onresolution: (width: number, height: number) => void;
    onfps: (fps: number) => void;
    /** 주사율 칩 아래 한 줄(모니터 주사율 안내) */
    fpsNote?: string | null;
    /** 칩 아래 마지막 줄(인코더·출력 크기) */
    note?: string | null;
  } = $props();

  const resolutionOptions = RESOLUTIONS.map((r) => ({ value: r.value, label: r.label }));
  const fpsOptions = FRAME_RATES.map((f) => ({ value: f.value, label: f.label }));

  function pickResolution(v: string) {
    const r = RESOLUTIONS.find((x) => x.value === v);
    if (r) onresolution(r.width, r.height);
  }
</script>

<div class="pair">
  <div class="group">
    <span class="label">해상도</span>
    <Chips options={resolutionOptions} value={resolutionValue(width, height)} label="해상도" onchange={pickResolution} />
  </div>
  <div class="group">
    <span class="label">주사율 <span class="unit">(fps)</span></span>
    <Chips options={fpsOptions} value={String(fps)} label="주사율" onchange={(v) => onfps(Number(v))} />
  </div>
  {#if fpsNote}<p class="hint wide">{fpsNote}</p>{/if}
  {#if note}<p class="hint wide num">{note}</p>{/if}
</div>

<style>
  .pair {
    display: flex;
    flex-wrap: wrap;
    column-gap: 20px;
    row-gap: 8px;
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .wide {
    flex-basis: 100%;
  }

  .unit {
    font-weight: 400;
    color: var(--text-3);
  }
</style>
