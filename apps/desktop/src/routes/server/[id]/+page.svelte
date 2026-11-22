<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { page } from "$app/state";
  import { splitMessage } from "$lib/errors";
  import { formatAge, secondsSince } from "$lib/format";
  import {
    addrLine,
    broadcastCount,
    downView,
    fingerprintPrefix,
    publishHref,
    serverScreen,
    tileHref,
    tileMeta,
  } from "$lib/serverScreen";
  import { conn, connect, connectSoon, isConnected, statusOf } from "$lib/stores/connections.svelte";
  import { serverById, serverStore } from "$lib/stores/servers.svelte";
  import BroadcastTile from "$lib/components/BroadcastTile.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Spinner from "$lib/components/Spinner.svelte";

  const id = $derived(page.params.id ?? "");
  const server = $derived(serverById(id));
  const live = $derived(conn.live[id]);
  const status = $derived(statusOf(id));
  const screen = $derived(serverScreen({ listLoaded: serverStore.loaded, exists: server !== undefined, status: status.kind }));
  const down = $derived(downView(status));
  const canPublish = $derived(live?.canPublish === true);
  /** 뒤에서 다시 시도하는 중이면 "다시 연결"을 막는다(같은 시도를 기다리게 된다). */
  const busy = $derived("retrying" in status && status.retrying);

  /** 방송 시간("30분")을 다시 계산하는 기준 시각 */
  let now = $state(Date.now());

  // 서버 화면에 들어오면(탭을 누름, 주소로 바로 들어옴) 연결 안 된 서버는 기다리지 않고 바로 다시 시도한다(조용히).
  // 사용자가 끊은 서버와 지문 경고가 있는 서버는 그대로 둔다(D-071). 서버마다 들어올 때 한 번만 본다.
  let enteredFor: string | null = null;
  $effect(() => {
    const current = id;
    if (enteredFor === current || !server) return;
    enteredFor = current;
    untrack(() => {
      if (!isConnected(current)) connectSoon([current]);
      // 연결돼 있다고 알려졌지만(앱 새로 고침 뒤 백엔드 목록) 서버 상태를 아직 못 읽은 경우는 읽어 온다.
      else if (!conn.live[current]) void connect(current);
    });
  });

  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(timer);
  });

  /** "다시 연결": 사용자가 직접 연결한다. 실패하면 이 화면과 탭에 빨갛게 보인다(D-054). */
  function reconnect() {
    void connect(id);
  }

  /** 지문 경고 아래 안내(백엔드 문장의 두 번째 문장부터: "서버를 다시 설치했다면 …") */
  const fingerprintHint = $derived(status.kind === "fingerprint" ? splitMessage(status.message).detail : null);

  const missing = splitMessage("서버를 찾을 수 없습니다. 목록에서 삭제되었을 수 있습니다.");
</script>

<div class="page">
  {#if screen === "loading"}
    <!-- 서버 목록을 읽는 동안 -->
  {:else if screen === "missing"}
    <EmptyState icon="server" title={missing.title} description={missing.detail ?? undefined}>
      {#snippet actions()}
        <a class="btn primary" href="#/">홈으로</a>
      {/snippet}
    </EmptyState>
  {:else if server}
    <div class="stack">
      <div class="head">
        <div class="titles">
          <h1 class="page-title">{server.name}</h1>
          <span class="sub">
            <span class="mono addrs">{addrLine(server.addrs)}</span>
            <span aria-hidden="true">·</span>
            {#if status.kind === "fingerprint"}
              <span class="fp warn"><Icon name="shield" size={14} />지문 다름</span>
            {:else if server.fingerprint}
              <span class="fp" title={server.fingerprint.full}><Icon name="shield" size={14} />지문 확인됨</span>
            {:else}
              <span class="fp warn"><Icon name="shield" size={14} />첫 접속 전</span>
            {/if}
          </span>
        </div>
        <!-- 연결 끊기·다시 연결·이름 변경·삭제는 위쪽 탭 메뉴에만 둔다(D-079). -->
        {#if screen === "live" && canPublish}
          <div class="tools">
            <a class="btn" href={publishHref(id)}><Icon name="broadcast" size={15} />이 서버에서 방송</a>
          </div>
        {/if}
      </div>

      {#if screen === "live"}
        <section class="stack" style="--gap: 14px" aria-labelledby="live-title">
          <div class="section-head">
            <h2 id="live-title">지금 방송 중</h2>
            {#if live}<span class="sub">{broadcastCount(live.broadcasts.length)}</span>{/if}
            <span class="spacer"></span>
          </div>

          {#if !live}
            <div class="connecting" role="status">
              <Spinner size={36} />
              <p>서버에 연결하는 중…</p>
            </div>
          {:else if live.broadcasts.length > 0}
            <ul class="grid">
              {#each live.broadcasts as b (b.broadcasterId)}
                <li>
                  <BroadcastTile
                    seed={b.broadcasterId}
                    name={b.broadcasterName}
                    href={tileHref(id, b)}
                    elapsed={formatAge(secondsSince(b.startedAt, now))}
                    locked={b.locked}
                    viewers={b.viewers}
                    mine={b.mine}
                    passwordSaved={b.locked && b.passwordSaved}
                    meta={tileMeta(b)}
                  />
                </li>
              {/each}
            </ul>
          {:else}
            <EmptyState
              icon="broadcast"
              title="지금 진행 중인 방송이 없습니다."
              description="누군가 방송을 시작하면 여기에 바로 나타납니다."
            >
              {#snippet actions()}
                {#if canPublish}
                  <a class="btn primary" href={publishHref(id)}><span class="dot" aria-hidden="true"></span>이 서버에서 방송</a>
                {/if}
              {/snippet}
            </EmptyState>
          {/if}
        </section>
      {:else if screen === "connecting"}
        <div class="connecting" role="status">
          <Spinner size={36} />
          <p>서버에 연결하는 중…</p>
        </div>
      {:else if status.kind === "fingerprint"}
        <!-- 인증서 지문이 다르다(D-054): 숨기지 않고 저장·실제 지문을 보인다. 확인하기 전에는 자동으로 다시 시도하지 않는다. -->
        <div class="mismatch" role="alert">
          <div class="inner">
            <span class="mismatch-icon" aria-hidden="true"><Icon name="shield" size={22} /></span>
            <p class="mismatch-title">인증서 지문이 다릅니다</p>
            {#if status.compare}
              <p class="desc">서버가 바뀌었거나 다른 서버일 수 있습니다. 확인하기 전에는 접속하지 않습니다.</p>
              <p class="mono compare">
                저장 <span title={status.compare.stored.full}>{fingerprintPrefix(status.compare.stored.short)}…</span> → 실제
                <span class="actual" title={status.compare.actual.full}>{fingerprintPrefix(status.compare.actual.short)}…</span>
              </p>
              {#if fingerprintHint}<p class="desc">{fingerprintHint}</p>{/if}
            {:else}
              <p class="desc">{status.message}</p>
            {/if}
            <div class="actions">
              <button type="button" class="btn" onclick={reconnect}><Icon name="refresh" size={15} />다시 연결</button>
            </div>
          </div>
        </div>
      {:else if down}
        <!-- 연결 안 됨: 머리는 그대로 두고 방송 목록 자리에 이유와 다시 연결(D-055, D-073) -->
        <div role={down.tone === "danger" || status.kind === "paused" ? undefined : "alert"}>
          <EmptyState icon={down.icon} tone={down.tone} title={down.title} description={down.detail ?? undefined}>
            {#snippet actions()}
              <button type="button" class="btn primary" disabled={busy} aria-busy={busy} onclick={reconnect}>
                <Icon name="refresh" size={15} />다시 연결
              </button>
            {/snippet}
          </EmptyState>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .head {
    display: flex;
    align-items: flex-end;
    gap: 16px;
    flex-wrap: wrap;
  }

  .titles {
    flex: 1 1 320px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .sub {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    font-size: 13px;
    color: var(--text-3);
  }

  .addrs {
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .fp {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .fp.warn {
    color: var(--amber-text);
  }

  .tools {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 20px;
  }

  .connecting {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 96px 0;
    font-size: 14px;
    color: var(--text-2);
  }

  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--red-dot);
  }
  .mismatch {
    display: grid;
    place-items: center;
    min-height: 300px;
    padding: 32px;
    border-radius: var(--r-empty);
    background: rgba(240, 180, 41, 0.06);
    box-shadow: inset 0 0 0 1px rgba(240, 180, 41, 0.45);
  }

  .mismatch .inner {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    max-width: 480px;
    text-align: center;
  }

  .mismatch-icon {
    width: 52px;
    height: 52px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgba(240, 180, 41, 0.14);
    color: var(--amber-text);
  }

  .mismatch-title {
    margin-top: 4px;
    font-size: 16px;
    font-weight: 650;
    color: var(--amber-text);
  }

  .mismatch .desc {
    font-size: 13px;
    color: var(--text-2);
    text-wrap: pretty;
  }

  .compare {
    font-size: 13px;
    overflow-wrap: anywhere;
  }

  .actual {
    color: var(--amber-text);
  }

  .mismatch .actions {
    display: flex;
    gap: 8px;
    margin-top: 6px;
  }
</style>
