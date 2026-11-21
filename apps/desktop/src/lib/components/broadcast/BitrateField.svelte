<script lang="ts">
  import { kbpsToMbps, normalizeKbps } from "$lib/bitrate";
  import {
    SLIDER_STEPS,
    ZONE_LABEL,
    bitrateWarning,
    mbpsText,
    sliderKbps,
    sliderPos,
    zoneOf,
    type BitrateZones,
  } from "$lib/bitrateZones";
  import type { HostingKind } from "$lib/types";
  import BitrateInput from "./BitrateInput.svelte";
  import Notice from "./Notice.svelte";

  // 비트레이트: 슬라이드 바(로그 눈금, 부족·적당·충분 색 띠) + 오른쪽의 작은 Mbps 입력란 + 구간 경고(D-074).
  let {
    id,
    value = $bindable(),
    zones,
    hosting,
    viewers = null,
    ontouch,
    oncommit,
  }: {
    id: string;
    /** kbps. 입력란이 비면 null. */
    value?: number | null;
    zones: BitrateZones;
    /** 경고 문구를 호스팅 방식에 맞춘다. */
    hosting: HostingKind | null;
    /** 방송 중이면 지금 시청자 수 */
    viewers?: number | null;
    /** 사용자가 값을 움직였다(바를 끌거나 숫자를 침). */
    ontouch?: () => void;
    /** 값을 확정했다(바를 놓음, 입력란에서 포커스가 빠지거나 Enter). 값은 허용 범위로 맞춘 뒤다. */
    oncommit?: (kbps: number) => void;
  } = $props();

  const shown = $derived(typeof value === "number" && value > 0 ? value : null);
  const zone = $derived(shown === null ? null : zoneOf(shown, zones));
  const pos = $derived(Math.round(sliderPos(shown ?? zones.recommended, zones)));
  const lowAt = $derived(sliderPos(zones.low, zones) / SLIDER_STEPS);
  const highAt = $derived(sliderPos(zones.high, zones) / SLIDER_STEPS);
  const warning = $derived(zone && shown !== null ? bitrateWarning(zone, shown, hosting, viewers) : null);
  const valueText = $derived(
    shown === null ? "비어 있음" : `${kbpsToMbps(shown)?.toFixed(2)} Mbps, ${ZONE_LABEL[zone ?? "ok"]}`,
  );

  function slide(e: Event) {
    value = sliderKbps(Number((e.currentTarget as HTMLInputElement).value), zones);
    ontouch?.();
  }

  function commit() {
    const kbps = normalizeKbps(value, zones.recommended);
    value = kbps;
    oncommit?.(kbps);
  }
</script>

<div class="bitrate">
  <div class="head">
    <label class="label" for={id}>비트레이트</label>
    {#if zone}
      <span class="badge {zone === 'ok' ? 'ok' : 'warn'}">{ZONE_LABEL[zone]}</span>
    {/if}
    <span class="hint num range">적당 {mbpsText(zones.low)}~{mbpsText(zones.high)} Mbps</span>
  </div>
  <div class="row">
    <div class="track" style:--low={lowAt} style:--high={highAt}>
      <input
        class="bar"
        type="range"
        min="0"
        max={SLIDER_STEPS}
        step="1"
        value={pos}
        aria-label="비트레이트 조절 막대"
        aria-valuetext={valueText}
        oninput={slide}
        onchange={commit}
      />
      <div class="zones" aria-hidden="true">
        <span class:on={zone === "low"} style:width="{lowAt * 100}%">{ZONE_LABEL.low}</span>
        <span class:on={zone === "ok"} style:width="{(highAt - lowAt) * 100}%">{ZONE_LABEL.ok}</span>
        <span class:on={zone === "high"} style:width="{(1 - highAt) * 100}%">{ZONE_LABEL.high}</span>
      </div>
    </div>
    <span class="field"><BitrateInput {id} bind:value oninput={() => ontouch?.()} oncommit={commit} /></span>
  </div>
  {#if warning}
    <Notice tone="warn" icon={zone === "low" ? "alert" : "upload"} size={12}>{warning}</Notice>
  {/if}
</div>

<style>
  .bitrate {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .range {
    margin-left: auto;
  }

  .row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }

  .track {
    /* 손잡이(16px) 가운데가 닿는 곳이 0~100%다. 색 띠와 글자를 손잡이 이동 폭에 맞춘다. */
    --thumb: 16px;
    --low-at: calc(var(--thumb) / 2 + (100% - var(--thumb)) * var(--low));
    --high-at: calc(var(--thumb) / 2 + (100% - var(--thumb)) * var(--high));
    flex: 1;
    min-width: 0;
    padding-top: 7px;
  }

  .bar {
    -webkit-appearance: none;
    appearance: none;
    display: block;
    width: 100%;
    height: 22px;
    margin: 0;
    background: transparent;
    cursor: pointer;
  }

  .bar::-webkit-slider-runnable-track {
    height: 6px;
    border-radius: 3px;
    background: linear-gradient(
      to right,
      rgba(240, 180, 41, 0.75) 0 var(--low-at),
      rgba(76, 195, 138, 0.8) var(--low-at) var(--high-at),
      rgba(240, 180, 41, 0.75) var(--high-at) 100%
    );
  }

  .bar::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: var(--thumb);
    height: var(--thumb);
    margin-top: -5px;
    border-radius: 50%;
    background: var(--text);
    box-shadow:
      0 0 0 3px var(--s1),
      0 1px 4px rgba(0, 0, 0, 0.5);
  }

  .bar:focus-visible {
    outline: none;
  }

  .bar:focus-visible::-webkit-slider-thumb {
    outline: var(--focus-ring);
    outline-offset: 2px;
  }

  .zones {
    display: flex;
    padding: 0 calc(var(--thumb) / 2);
    margin-top: 2px;
    font-size: 11px;
    color: var(--text-3);
    white-space: nowrap;
  }

  .zones span {
    flex: none;
    overflow: hidden;
    text-align: center;
    text-overflow: clip;
  }

  .zones span.on {
    color: var(--text);
    font-weight: 600;
  }

  .field {
    flex: none;
    width: 118px;
    margin-top: 0;
  }
</style>
