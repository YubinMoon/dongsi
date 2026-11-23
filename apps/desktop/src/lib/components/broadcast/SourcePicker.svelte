<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "../Icon.svelte";
  import SourceTile from "./SourceTile.svelte";
  import { groupSources } from "$lib/broadcast";
  import type { SourceThumbnail, SourceView } from "$lib/types";

  let {
    sources,
    thumbnails = {},
    value,
    onselect,
    onrefresh,
    refreshing = false,
    actions,
  }: {
    sources: SourceView[];
    /** 소스 id별 미리 보기 */
    thumbnails?: Record<string, SourceThumbnail>;
    /** 고른 소스 id */
    value: string;
    onselect: (id: string) => void;
    onrefresh: () => void;
    refreshing?: boolean;
    /** 머리 줄의 "새로 고침" 오른쪽에 붙는 단추(방송 중 화면 전환의 취소·바꾸기) */
    actions?: Snippet;
  } = $props();

  const uid = $props.id();
  const groups = $derived(groupSources(sources));
</script>

<section class="sources" aria-labelledby="{uid}-title">
  <div class="head">
    <h2 id="{uid}-title">캡처 대상</h2>
    <span class="spacer"></span>
    <button type="button" class="btn sm" onclick={onrefresh} disabled={refreshing}>
      <Icon name="refresh" size={14} />새로 고침
    </button>
    {@render actions?.()}
  </div>

  {#if groups.length === 0}
    <p class="empty">캡처할 수 있는 대상이 없습니다.</p>
  {:else}
    <div class="groups" role="radiogroup" aria-labelledby="{uid}-title">
      {#each groups as g (g.kind)}
        <h3>{g.label}</h3>
        <div class="grid">
          {#each g.items as s (s.id)}
            <SourceTile
              source={s}
              thumbnail={thumbnails[s.id]}
              name="{uid}-source"
              selected={s.id === value}
              {onselect}
            />
          {/each}
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .sources {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    min-height: 32px;
  }

  h2 {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .spacer {
    flex: 1;
  }

  .groups {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  h3 {
    font-size: 12px;
    font-weight: 600;
    line-height: 1.5;
    color: var(--text-3);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: 16px 14px;
    /* 고른 소스의 바깥 테두리(4px)가 잘리지 않게 */
    padding: 4px;
    margin: -4px;
  }

  .empty {
    font-size: 13px;
    color: var(--text-3);
  }
</style>
