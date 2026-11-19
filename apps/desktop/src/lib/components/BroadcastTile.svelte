<script lang="ts">
  import { hueFor, tileVars } from "$lib/hue";
  import { initial } from "$lib/format";
  import Icon from "./Icon.svelte";

  let {
    seed,
    name,
    href,
    onclick,
    elapsed = null,
    via = null,
    locked = false,
    viewers = null,
    meta,
    mine = false,
    passwordSaved = false,
    live = true,
  }: {
    /** 색조를 뽑는 문자열(방송자 ID). 같은 사람은 늘 같은 색. */
    seed: string;
    name: string;
    /** 있으면 링크, 없고 onclick이 있으면 단추, 둘 다 없으면 미리 보기용 div */
    href?: string;
    onclick?: () => void;
    /** LIVE 옆 방송 시간 알약. 예: "30분" */
    elapsed?: string | null;
    /** LIVE 옆 경로 알약. 예: "LAN" */
    via?: string | null;
    locked?: boolean;
    /** 오른쪽 아래 시청자 수. null이면 숨긴다. */
    viewers?: number | null;
    /** 아래 줄. 예: "1080p · 60fps · H.264" */
    meta: string;
    mine?: boolean;
    passwordSaved?: boolean;
    live?: boolean;
  } = $props();

  const style = $derived(tileVars(hueFor(seed)));
  const tag = $derived(href ? "a" : onclick ? "button" : "div");
</script>

<!-- onclick은 tag가 button일 때만 붙는다(위 tag 계산). 정적 요소 검사는 이를 알지 못해 끈다. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<svelte:element
  this={tag}
  class="tile"
  class:interactive={tag !== "div"}
  {style}
  href={tag === "a" ? href : undefined}
  type={tag === "button" ? "button" : undefined}
  onclick={tag === "button" ? onclick : undefined}
>
  <span class="art">
    <span class="avatar" aria-hidden="true">{initial(name)}</span>
    <span class="top-left">
      {#if live}<span class="badge live">LIVE</span>{/if}
      {#if via}<span class="badge dark">{via}</span>{/if}
      {#if elapsed}<span class="badge dark">{elapsed}</span>{/if}
    </span>
    {#if locked}
      <span class="top-right">
        <span class="badge dark lock"><Icon name="lock" size={12} /><span class="sr-only">비밀번호 필요</span></span>
      </span>
    {/if}
    {#if viewers !== null}
      <span class="bottom-right">
        <span class="badge dark">
          <Icon name="eye" size={12} /><span aria-hidden="true">{viewers}명</span>
          <span class="sr-only">{viewers}명 시청 중</span>
        </span>
      </span>
    {/if}
  </span>
  <span class="info">
    <span class="row">
      <span class="name">{name}</span>
      {#if mine}<span class="tag">내 방송</span>{/if}
      {#if passwordSaved}<span class="saved">비밀번호 기억됨</span>{/if}
    </span>
    <span class="meta">{meta}</span>
  </span>
</svelte:element>

<style>
  .tile {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
    padding: 0;
    color: inherit;
    text-align: left;
  }

  .art {
    position: relative;
    display: grid;
    place-items: center;
    aspect-ratio: 16 / 9;
    border-radius: var(--r-tile);
    overflow: hidden;
    background-color: oklch(0.25 0.035 var(--h));
    background-image: radial-gradient(rgba(255, 255, 255, 0.07) 1px, transparent 1.3px);
    background-size: 14px 14px;
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
    transition: background-color 0.15s;
  }

  .interactive {
    cursor: pointer;
  }

  .interactive:hover .art {
    background-color: oklch(0.285 0.04 var(--h));
  }

  .interactive:focus-visible {
    outline: none;
  }

  .interactive:focus-visible .art {
    outline: var(--focus-ring);
    outline-offset: 2px;
  }

  .avatar {
    width: 60px;
    height: 60px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: oklch(0.72 0.11 var(--h));
    color: oklch(0.22 0.04 var(--h));
    font-size: 24px;
    font-weight: 700;
    line-height: 1;
  }

  .top-left {
    position: absolute;
    top: 10px;
    left: 10px;
    display: flex;
    gap: 6px;
  }

  .top-right {
    position: absolute;
    top: 10px;
    right: 10px;
  }

  .bottom-right {
    position: absolute;
    bottom: 10px;
    right: 10px;
  }

  .lock {
    padding: 0 6px;
  }

  .info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .name {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .saved {
    flex: none;
    font-size: 12px;
    color: var(--text-3);
  }

  .meta {
    font-size: 13px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
