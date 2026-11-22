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
    /* 줄이 많아져 화면 높이를 넘으면 패널 안에서 스크롤한다 */
    max-height: calc(100% - 66px - 96px);
    overflow-y: auto;
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
