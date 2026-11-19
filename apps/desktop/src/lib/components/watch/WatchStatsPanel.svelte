<script lang="ts">
  import type { StatRow } from "$lib/watch";

  let { rows }: { rows: StatRow[] } = $props();
</script>

<!-- 통계 패널: 오른쪽 위 막대 아래. 숫자는 고정폭. -->
<section class="w-panel stats" aria-label="통계">
  <dl>
    {#each rows as row (row.label)}
      <dt>{row.label}</dt>
      <dd>{row.value}</dd>
    {/each}
  </dl>
</section>

<style>
  .stats {
    position: absolute;
    top: 66px;
    right: 12px;
    z-index: 2;
    width: 290px;
    padding: 14px 16px;
  }

  dl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 7px 16px;
    margin: 0;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  dt {
    color: var(--text-3);
  }

  dd {
    margin: 0;
    text-align: right;
    font-family: var(--font-mono);
    color: #ffffff;
    overflow-wrap: anywhere;
  }
</style>
