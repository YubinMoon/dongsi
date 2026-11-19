<script lang="ts">
  let { size = 36, label }: { size?: number; label?: string } = $props();
</script>

<!-- 시안의 원형 회전(b-home-connecting, b-watch-connecting). label이 있으면 화면 읽기용 문구가 된다. -->
<span class="spinner" style:width="{size}px" style:height="{size}px" role={label ? "status" : undefined} aria-label={label}
></span>

<style>
  .spinner {
    display: block;
    flex: none;
    box-sizing: border-box;
    border-radius: 50%;
    border: 3px solid var(--line-input);
    border-top-color: #ffffff;
    animation: spin 0.9s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }

  /* 움직임을 줄이는 설정이면 돌지 않고 천천히 깜빡인다. */
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: pulse 1.8s ease-in-out infinite;
    }
  }
</style>
