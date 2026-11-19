<script lang="ts">
  import { ICONS, type IconName, type IconShape } from "./icons";

  let {
    name,
    size = 16,
    strokeWidth = 1.8,
  }: {
    name: IconName;
    size?: number;
    strokeWidth?: number;
  } = $props();

  const shape: IconShape = $derived(ICONS[name]);
</script>

<!-- 장식용이다. 뜻이 필요하면 바깥 단추·링크에 aria-label을 단다. 경로는 icons.ts의 고정 문자열이다. -->
<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill={shape.fill ? "currentColor" : "none"}
  stroke={shape.fill ? "none" : "currentColor"}
  stroke-width={strokeWidth}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
  style="flex: none"
>
  {@html shape.body}
</svg>
