<script lang="ts">
  import type { Snippet } from "svelte";
  import { tileVars } from "$lib/hue";

  let {
    hue,
    dim = false,
    children,
  }: {
    /** 방송자 색조(0~359) */
    hue: number;
    /** 끝남·오류 화면처럼 더 어둡게 */
    dim?: boolean;
    children: Snippet;
  } = $props();
</script>

<!-- 영상 자리를 덮는 방송자 색조의 점 무늬 바탕(시안: 연결하는 중·비밀번호·끝남·오류). 위 막대보다 아래에 깔린다. -->
<div class="backdrop" style={tileVars(hue)}>
  {#if dim}<span class="dim" aria-hidden="true"></span>{/if}
  {@render children()}
</div>

<style>
  .backdrop {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 80px 24px;
    overflow: auto;
    background-color: oklch(0.25 0.035 var(--h));
    background-image: radial-gradient(rgba(255, 255, 255, 0.05) 1px, transparent 1.3px);
    background-size: 14px 14px;
  }

  .dim {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
  }

  .backdrop > :global(:not(.dim)) {
    position: relative;
  }
</style>
