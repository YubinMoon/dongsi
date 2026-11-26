<script lang="ts">
  import { sparklineGeometry } from "$lib/watch";

  let {
    values,
    width = 64,
    height = 18,
    tone = "default",
    label = "",
  }: {
    values: readonly (number | null)[];
    width?: number;
    height?: number;
    tone?: "default" | "ok" | "warn" | "err";
    label?: string;
  } = $props();

  const geom = $derived(sparklineGeometry(values, width, height));
</script>

<svg
  class="spark {tone}"
  {width}
  {height}
  viewBox="0 0 {width} {height}"
  role={label ? "img" : undefined}
  aria-label={label || undefined}
  aria-hidden={label ? undefined : true}
>
  {#if geom}
    <polygon class="area" points={geom.area} />
    <polyline class="line" points={geom.line} />
  {:else}
    <line class="empty" x1="1" y1={height / 2} x2={width - 1} y2={height / 2} />
  {/if}
</svg>

<style>
  .spark {
    flex: none;
    display: block;
    overflow: visible;
    color: rgba(255, 255, 255, 0.82);
  }

  .spark.ok {
    color: var(--green);
  }

  .spark.warn {
    color: var(--amber);
  }

  .spark.err {
    color: var(--red-dot-2);
  }

  .line {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .area {
    fill: currentColor;
    opacity: 0.16;
  }

  .empty {
    stroke: rgba(255, 255, 255, 0.22);
    stroke-width: 1;
    stroke-dasharray: 2 2;
  }
</style>
