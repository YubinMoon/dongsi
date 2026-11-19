<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "$lib/components/Icon.svelte";

  let text = $state<string | null>(null);
  let error = $state<string | null>(null);

  onMount(async () => {
    try {
      // 저장소 루트의 생성물(scripts/licenses/generate.mjs). 크기가 커서 이 화면을 열 때만 불러온다.
      const mod = await import("../../../../../../THIRD_PARTY_NOTICES.md?raw");
      text = mod.default;
    } catch {
      error = "제3자 고지를 불러오지 못했습니다.";
    }
  });
</script>

<div class="page narrow stack" style="--gap: 16px">
  <div class="head">
    <a class="btn ghost sm back" href="#/settings"><Icon name="arrow-left" size={15} />설정</a>
    <h1 class="page-title title">제3자 고지</h1>
  </div>
  <p class="desc">
    dongsi가 사용하는 FFmpeg, 글꼴, 라이브러리의 라이선스 고지입니다. 같은 내용이 포터블 폴더의 THIRD_PARTY_NOTICES.md에도 있습니다.
  </p>

  {#if text !== null}
    <!-- 긴 문서라 키보드로도 스크롤할 수 있게 tabindex를 준다. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <pre class="notices mono" tabindex="0" aria-label="제3자 고지 전문">{text}</pre>
  {:else if error}
    <p class="error" role="alert">{error}</p>
  {:else}
    <p class="loading">불러오는 중…</p>
  {/if}
</div>

<style>
  .narrow {
    max-width: 960px;
    padding-top: 28px;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .back {
    padding-left: 8px;
  }

  .title {
    margin: 0;
  }

  .desc {
    font-size: 13px;
    color: var(--text-3);
    text-wrap: pretty;
  }

  .notices {
    margin: 0;
    height: calc(100vh - 230px);
    min-height: 260px;
    overflow: auto;
    padding: 18px 20px;
    border-radius: var(--r-panel);
    background: var(--s1);
    box-shadow: inset 0 0 0 1px var(--line);
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    word-break: normal;
    user-select: text;
  }

  .error {
    font-size: 13px;
    color: var(--red-text);
  }

  .loading {
    color: var(--text-2);
  }
</style>
