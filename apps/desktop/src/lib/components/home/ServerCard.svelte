<script lang="ts">
  import Icon from "../Icon.svelte";
  import Menu from "../Menu.svelte";
  import Spinner from "../Spinner.svelte";
  import { addrSummary } from "$lib/format";
  import { cardClickAction, cardStatus, fingerprintPrefix, isHomeHash } from "$lib/home";
  import { conn, connect, isConnected } from "$lib/stores/connections.svelte";
  import type { ServerView } from "$lib/types";

  let {
    server,
    onrename,
    onremove,
  }: {
    server: ServerView;
    onrename: (server: ServerView) => void;
    onremove: (server: ServerView) => void;
  } = $props();

  const connected = $derived(isConnected(server.id));
  const status = $derived(cardStatus(server, connected, conn.card[server.id]));

  /** 카드를 누르면(D-054): 연결돼 있으면 바로 서버 화면, 아니면 연결하고 성공하면 간다. 실패는 카드에 보인다. */
  async function activate() {
    const action = cardClickAction(connected, conn.card[server.id]);
    if (action === "none") return;
    if (action === "connect") {
      const live = await connect(server.id);
      if (!live) return;
      // 연결하는 동안 다른 화면으로 옮겼다면 끌어오지 않는다.
      if (!isHomeHash(location.hash)) return;
    }
    location.hash = `#/server/${server.id}`;
  }
</script>

<div
  class="card"
  class:connecting={status.kind === "connecting"}
  class:failed={status.kind === "failed"}
  class:mismatch={status.kind === "fingerprint"}
  data-server-id={server.id}
>
  <div class="head">
    <span class="icon" aria-hidden="true"><Icon name="server" size={18} /></span>
    <button
      type="button"
      class="main"
      aria-disabled={status.kind === "connecting" ? "true" : undefined}
      onclick={activate}
    >
      <span class="name">{server.name}</span>
      <span class="addr mono">{addrSummary(server.addrs)}</span>
    </button>
    <Menu
      label="'{server.name}' 서버 메뉴"
      items={[
        { label: "이름 변경", icon: "pencil", onselect: () => onrename(server) },
        { label: "삭제", icon: "trash", danger: true, onselect: () => onremove(server) },
      ]}
    />
  </div>

  <div class="status">
    {#if status.kind === "connecting"}
      <span class="connecting-text" role="status">
        <Spinner size={12} />연결하는 중…
      </span>
    {:else if status.kind === "failed"}
      <span class="fail" role="alert" title={status.title}>
        <Icon name="alert" size={14} />{status.message}
      </span>
      <button type="button" class="btn sm retry" onclick={activate}>
        <Icon name="refresh" size={13} />다시 연결
      </button>
    {:else if status.kind === "fingerprint"}
      <div class="mismatch-box" role="alert">
        <span class="mismatch-title"><Icon name="shield" size={14} />인증서 지문이 다릅니다</span>
        {#if status.compare}
          <span class="mismatch-text">서버가 바뀌었거나 다른 서버일 수 있습니다. 확인하기 전에는 접속하지 않습니다.</span>
          <span class="mono compare">
            저장 <span title={status.compare.stored.full}>{fingerprintPrefix(status.compare.stored.short)}…</span> → 실제
            <span class="actual" title={status.compare.actual.full}>{fingerprintPrefix(status.compare.actual.short)}…</span>
          </span>
        {:else}
          <span class="mismatch-text">{status.message}</span>
        {/if}
        <span class="retry-row">
          <button type="button" class="btn sm retry" onclick={activate}>
            <Icon name="refresh" size={13} />다시 연결
          </button>
        </span>
      </div>
    {:else if status.kind === "firstTime"}
      <span class="first">
        <span class="dot amber" aria-hidden="true"></span>첫 접속 전 · 지문은 처음 접속할 때 확인
      </span>
    {:else}
      {#if status.kind === "connected"}
        <span class="ok"><span class="dot green" aria-hidden="true"></span>연결됨</span>
      {/if}
      {#if status.kind === "known" || status.fingerprinted}
        <span class="chip"><Icon name="shield" size={13} />지문 확인됨</span>
      {/if}
      {#if status.savedPasswords > 0}
        <span class="chip"><Icon name="key" size={13} />비밀번호 {status.savedPasswords}개 기억</span>
      {/if}
    {/if}
  </div>
</div>

<style>
  .card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-width: 0;
    padding: 16px;
    border-radius: var(--r-tile);
    background: var(--s1);
    box-shadow: inset 0 0 0 1px var(--line);
    transition: box-shadow 0.12s;
  }

  .card:hover,
  .card.connecting {
    box-shadow: inset 0 0 0 1px var(--line-strong);
  }

  .card.failed {
    background: rgba(229, 56, 59, 0.06);
    box-shadow: inset 0 0 0 1px rgba(229, 56, 59, 0.4);
  }

  .card.mismatch {
    background: rgba(240, 180, 41, 0.06);
    box-shadow: inset 0 0 0 1px rgba(240, 180, 41, 0.45);
  }

  .head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    min-width: 0;
  }

  .icon {
    flex: none;
    width: 40px;
    height: 40px;
    border-radius: var(--r-box);
    display: grid;
    place-items: center;
    background: var(--s2);
    color: var(--text-2);
  }

  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    cursor: pointer;
  }

  .main[aria-disabled="true"] {
    cursor: progress;
  }

  /* 카드 어디를 눌러도 본문 단추가 눌리게 한다. 메뉴·다시 연결 단추는 position: relative로 그 위에 둔다. */
  .main::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: var(--r-tile);
  }

  .main:focus-visible {
    outline: none;
  }

  .main:focus-visible::after {
    outline: var(--focus-ring);
    outline-offset: 2px;
  }

  .name {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .addr {
    font-size: 12px;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--text-2);
  }

  .dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }

  .dot.green {
    background: var(--green);
  }

  .dot.amber {
    background: var(--amber);
  }

  .ok,
  .first {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .ok {
    color: var(--green-text);
    font-weight: 600;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }

  .connecting-text {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    color: var(--text);
  }

  .fail {
    flex: 1 1 160px;
    min-width: 0;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--red-text);
    font-weight: 600;
  }

  .fail :global(svg) {
    flex: none;
  }

  .mismatch-box {
    flex: 1 1 100%;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .mismatch-title {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--amber-text);
    font-size: 13px;
    font-weight: 650;
  }

  .mismatch-text {
    text-wrap: pretty;
  }

  .compare {
    overflow-wrap: anywhere;
  }

  .actual {
    color: var(--amber-text);
  }

  .retry-row {
    display: flex;
    margin-top: 4px;
  }

  .status .retry {
    position: relative;
    height: 28px;
  }
</style>
