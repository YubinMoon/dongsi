<script lang="ts">
  /** size = 가로 px. 세로는 그림 범위 비율(944:734)을 따른다. */
  let { size = 26 }: { size?: number } = $props();

  /** 한 페이지에 로고가 여러 개여도 그라데이션 id가 겹치지 않게 한다. */
  const uid = $props.id();
  const gradient = `${uid}-pu`;
</script>

<!-- icon.svg에서 옮김(src-tauri/icons/icon.svg '화면 눈'), viewBox만 그림 범위로 잘랐다. 장식용이다. -->
<svg
  width={size}
  height={(size * 734) / 944}
  viewBox="40 116 944 734"
  aria-hidden="true"
  focusable="false"
  style="flex: none"
>
  <defs>
    <linearGradient id={gradient} x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#8E5CFF" /><stop offset="1" stop-color="#6A2EE0" />
    </linearGradient>
  </defs>
  <path
    d="M296 325 L249 212 M512 276 L512 152 M728 325 L775 212"
    stroke="#8357FC"
    stroke-width="72"
    stroke-linecap="round"
    fill="none"
  />
  <path
    d="M60 560 C 230 160, 794 160, 964 560 C 794 920, 230 920, 60 560 Z"
    fill="url(#{gradient})"
    stroke="url(#{gradient})"
    stroke-width="40"
    stroke-linejoin="round"
  />
  <rect x="262" y="380" width="500" height="340" rx="92" fill="#B8F23A" />
  <circle cx="512" cy="550" r="106" fill="#3F1590" />
  <circle cx="468" cy="504" r="36" fill="#FFFFFF" />
</svg>
