<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  // 조언 한 줄(전구 아이콘 + 작은 말풍선 상자). 문구는 "~해 보아요" 말투로 쓴다(AGENTS.md "화면 문구").
  let { children }: { children: Snippet } = $props();
</script>

<p class="tip">
  <span class="icon"><Icon name="bulb" size={14} /></span>
  <span class="text">{@render children()}</span>
</p>

<style>
  .tip {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 8px 10px;
    border-radius: var(--r-btn);
    background: var(--s2);
    box-shadow: inset 0 0 0 1px var(--line);
    font-size: 12px;
    color: var(--text-2);
    text-wrap: pretty;
  }

  .icon {
    flex: none;
    display: inline-flex;
    margin-top: 1px;
    color: var(--amber-text);
  }

  .text {
    min-width: 0;
  }
</style>
