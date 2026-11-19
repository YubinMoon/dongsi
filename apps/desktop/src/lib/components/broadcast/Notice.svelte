<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "../Icon.svelte";
  import type { IconName } from "../icons";

  let {
    tone,
    icon,
    boxed = false,
    size = 13,
    children,
  }: {
    tone: "ok" | "warn" | "info";
    icon: IconName;
    /** 호박색 테두리·바탕 상자(포트 대체·이중 NAT 같은 안내) */
    boxed?: boolean;
    /** 글자 크기(13: 공유 칸, 12: 설정 안내 줄) */
    size?: 12 | 13;
    children: Snippet;
  } = $props();
</script>

<p class="note {tone}" class:boxed style:font-size="{size}px">
  <span class="icon" class:small={size === 12}><Icon name={icon} size={size === 12 ? 14 : 15} /></span>
  <span class="text">{@render children()}</span>
</p>

<style>
  .note {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    text-wrap: pretty;
  }

  .icon {
    flex: none;
    display: inline-flex;
    margin-top: 2px;
  }

  .icon.small {
    margin-top: 1px;
  }

  .ok {
    color: var(--green-text);
  }

  .warn {
    color: var(--amber-text);
  }

  .info {
    color: var(--text-3);
  }

  .boxed {
    gap: 8px;
    padding: 10px 12px;
    border-radius: var(--r-btn);
  }

  .boxed.warn {
    background: rgba(240, 180, 41, 0.1);
    box-shadow: inset 0 0 0 1px rgba(240, 180, 41, 0.3);
  }

  .text {
    min-width: 0;
  }
</style>
