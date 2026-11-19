<script lang="ts">
  import { splitMessage } from "$lib/errors";
  import { ENDED_MESSAGE, endedDescription } from "$lib/watch";
  import Icon from "../Icon.svelte";
  import Spinner from "../Spinner.svelte";

  let {
    kind,
    name,
    message = "",
    popout = false,
    backHref,
    onretry,
    onclose,
  }: {
    kind: "connecting" | "ended" | "error";
    /** 방송자 이름(끝남 화면의 설명에 쓴다) */
    name: string;
    /** 오류 문장. 첫 문장은 제목, 나머지는 설명으로 나눈다. */
    message?: string;
    popout?: boolean;
    backHref: string;
    onretry: () => void;
    onclose: () => void;
  } = $props();

  const parts = $derived(splitMessage(message));
</script>

{#if kind === "connecting"}
  <div class="inner" role="status">
    <Spinner size={36} />
    <p class="title">연결하는 중…</p>
  </div>
{:else if kind === "ended"}
  <div class="inner">
    <span class="icon" aria-hidden="true"><Icon name="square" size={22} /></span>
    <p class="title">{ENDED_MESSAGE}</p>
    <p class="desc">{endedDescription(name)}</p>
    <div class="actions">
      {#if popout}
        <button type="button" class="btn primary" onclick={onclose}>창 닫기</button>
      {:else}
        <a class="btn primary" href={backHref}>목록으로</a>
      {/if}
    </div>
  </div>
{:else}
  <div class="inner" role="alert">
    <span class="icon danger" aria-hidden="true"><Icon name="alert" size={22} /></span>
    <p class="title">{parts.title}</p>
    {#if parts.detail}<p class="desc">{parts.detail}</p>{/if}
    <div class="actions">
      <button type="button" class="btn primary" onclick={onretry}><Icon name="refresh" size={15} />다시 시도</button>
    </div>
  </div>
{/if}

<style>
  .inner {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    max-width: 440px;
    text-align: center;
  }

  .icon {
    width: 52px;
    height: 52px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgba(255, 255, 255, 0.08);
    color: #ffffff;
  }

  .icon.danger {
    background: rgba(229, 56, 59, 0.16);
    color: var(--red-text);
  }

  .title {
    font-size: 16px;
    font-weight: 600;
    letter-spacing: -0.01em;
    text-wrap: pretty;
  }

  .desc {
    margin-top: -6px;
    font-size: 13px;
    color: var(--text-2);
    text-wrap: pretty;
  }

  .actions {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
</style>
