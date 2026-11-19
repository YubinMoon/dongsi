<script lang="ts">
  import Icon from "../Icon.svelte";
  import { sourceSub } from "$lib/broadcast";
  import type { SourceView } from "$lib/types";

  let {
    source,
    name,
    selected,
    onselect,
  }: {
    source: SourceView;
    /** 라디오 그룹 이름(같은 이름끼리 화살표 키로 이동) */
    name: string;
    selected: boolean;
    onselect: (id: string) => void;
  } = $props();

  const icon = $derived(source.kind === "monitor" ? "monitor" : source.kind === "window" ? "window" : "sliders");
</script>

<label class="tile" class:on={selected}>
  <input type="radio" {name} value={source.id} checked={selected} onchange={() => onselect(source.id)} />
  <span class="art">
    <Icon name={icon} size={28} />
    {#if selected}
      <span class="check"><Icon name="check" size={14} /></span>
    {/if}
  </span>
  <span class="text">
    <span class="name" title={source.name}>{source.name}</span>
    <span class="sub">{sourceSub(source)}</span>
  </span>
</label>

<style>
  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    cursor: pointer;
  }

  input {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: 0;
    opacity: 0;
  }

  .art {
    position: relative;
    display: grid;
    place-items: center;
    aspect-ratio: 16 / 9;
    border-radius: var(--r-box);
    background: var(--s2);
    color: var(--text-3);
    box-shadow: inset 0 0 0 1px var(--line);
    transition: background-color 0.12s, color 0.12s;
  }

  .tile:hover:not(.on) .art {
    color: var(--text-2);
  }

  .on .art {
    background: var(--s3);
    color: var(--text);
    box-shadow: inset 0 0 0 1px var(--line), 0 0 0 2px var(--bg), 0 0 0 4px var(--text);
  }

  .tile:has(input:focus-visible) .art {
    outline: var(--focus-ring);
    outline-offset: 3px;
  }

  .on:has(input:focus-visible) .art {
    outline-offset: 7px;
  }

  .check {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--text);
    color: var(--bg);
  }

  .text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .name {
    font-size: 13px;
    font-weight: 550;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .on .name {
    font-weight: 650;
  }

  .sub {
    font-size: 12px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
