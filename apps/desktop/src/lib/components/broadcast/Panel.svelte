<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    title,
    size = "md",
    style,
    aside,
    children,
  }: {
    title: string;
    /** md: 방송 중 화면의 패널(제목 14/650), lg: 설정 화면의 패널(제목 15/600) */
    size?: "md" | "lg";
    /** 부모 격자에서 쓰는 flex 값 등 */
    style?: string;
    /** 제목 줄 오른쪽(배지 등) */
    aside?: Snippet;
    children: Snippet;
  } = $props();

  const uid = $props.id();
</script>

<section class="panel" class:lg={size === "lg"} {style} aria-labelledby="{uid}-title">
  <div class="head">
    <h2 id="{uid}-title">{title}</h2>
    <span class="spacer"></span>
    {#if aside}{@render aside()}{/if}
  </div>
  {@render children()}
</section>

<style>
  .panel {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px 20px 20px;
    border-radius: var(--r-panel);
    background: var(--s1);
    box-shadow: inset 0 0 0 1px var(--line);
  }

  .panel.lg {
    gap: 16px;
    padding: 20px 22px;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    min-height: 24px;
  }

  .lg .head {
    min-height: 0;
  }

  h2 {
    font-size: 14px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }

  .lg h2 {
    font-size: 15px;
    font-weight: 600;
    line-height: 1.5;
  }

  .spacer {
    flex: 1;
  }
</style>
