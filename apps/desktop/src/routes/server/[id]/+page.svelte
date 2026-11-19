<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { page } from "$app/state";
  import { closedReasonDetail, splitMessage } from "$lib/errors";
  import { formatAge, secondsSince } from "$lib/format";
  import { serverScreen, addrLine, broadcastCount, publishHref, tileMeta, watchHref } from "$lib/serverScreen";
  import { conn, connect, disconnect, dismissCard, isConnected, setViewing } from "$lib/stores/connections.svelte";
  import { serverById, serverStore } from "$lib/stores/servers.svelte";
  import BroadcastTile from "$lib/components/BroadcastTile.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Menu, { type MenuItem } from "$lib/components/Menu.svelte";
  import RemoveServerDialog from "$lib/components/RemoveServerDialog.svelte";
  import RenameServerDialog from "$lib/components/RenameServerDialog.svelte";
  import Spinner from "$lib/components/Spinner.svelte";

  const id = $derived(page.params.id ?? "");
  const server = $derived(serverById(id));
  const live = $derived(conn.live[id]);
  const screen = $derived(
    serverScreen({
      listLoaded: serverStore.loaded,
      exists: server !== undefined,
      connected: isConnected(id),
      closedTab: conn.tabs.closed.includes(id),
    }),
  );
  const canPublish = $derived(live?.canPublish === true);

  let renameOpen = $state(false);
  let removeOpen = $state(false);
  /** 끊긴 서버에 다시 연결하는 중 */
  let retrying = $state(false);
  /** 다시 연결이 실패했을 때 설명 줄에 보일 문장 */
  let retryError = $state<string | null>(null);
  /** 방송 시간("30분")을 다시 계산하는 기준 시각 */
  let now = $state(Date.now());

  // 이 화면이 보는 서버를 저장소에 알린다(보던 서버가 끊겨도 탭이 남게 하고, 다른 화면으로 가면 지운다).
  $effect(() => {
    const current = id;
    untrack(() => setViewing(current));
    return () => {
      untrack(() => {
        if (conn.viewing === current) setViewing(null);
      });
    };
  });

  // 연결 안 됐고 끊긴 탭도 아니면(주소로 바로 들어옴, 앱 새로 고침) 연결을 시도한다. 화면(서버)마다 한 번만.
  // 탭의 ×로 끊은 직후 다시 연결하지 않도록, 상태가 바뀔 때마다가 아니라 처음 한 번만 본다.
  let attemptedFor: string | null = null;
  $effect(() => {
    const current = id;
    if (attemptedFor === current || screen === "loading") return;
    attemptedFor = current;
    // 연결돼 있다고 알려졌지만(앱 새로 고침 뒤 백엔드 목록) 서버 상태를 아직 못 읽은 경우도 읽어 온다.
    if (screen !== "connecting" && !(screen === "live" && !live)) return;
    untrack(() => {
      void connect(current).then((result) => {
        if (result || id !== current) return;
        // 실패는 홈 서버 카드가 보인다(D-054).
        location.hash = "#/";
      });
    });
  });

  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(timer);
  });

  async function reconnect() {
    if (retrying) return;
    retrying = true;
    retryError = null;
    const key = id;
    const result = await connect(key);
    retrying = false;
    if (result) return;
    const card = conn.card[key];
    if (card?.kind === "fingerprint") {
      // 지문 경고는 홈 서버 카드가 보인다.
      location.hash = "#/";
      return;
    }
    retryError = card && card.kind === "failed" ? card.message : "서버에 연결하지 못했습니다.";
    dismissCard(key);
  }

  const menuItems: MenuItem[] = [
    { label: "이름 변경", icon: "pencil", onselect: () => (renameOpen = true) },
    { label: "연결 끊기", icon: "x", onselect: () => void disconnect(id) },
    { label: "삭제", icon: "trash", danger: true, onselect: () => (removeOpen = true) },
  ];

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
  {:else if screen === "connecting"}
    <div class="connecting" role="status">
      <Spinner size={36} />
      <p>서버에 연결하는 중…</p>
    </div>
  {:else if server}
    <div class="stack">
      <div class="head">
        <div class="titles">
          <h1 class="page-title">{server.name}</h1>
          <span class="sub">
            <span class="mono addrs">{addrLine(server.addrs)}</span>
            <span aria-hidden="true">·</span>
            {#if server.fingerprint}
              <span class="fp" title={server.fingerprint.full}><Icon name="shield" size={14} />지문 확인됨</span>
            {:else}
              <span class="fp warn"><Icon name="shield" size={14} />첫 접속 전</span>
            {/if}
          </span>
        </div>
        <div class="tools">
          {#if screen === "live" && canPublish}
            <a class="btn" href={publishHref(id)}><Icon name="broadcast" size={15} />이 서버에서 방송</a>
          {/if}
          <Menu label="서버 메뉴" size="md" items={menuItems} />
        </div>
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
                    href={watchHref(id, b.broadcasterId)}
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
      {:else}
        <!-- 보던 중에 연결이 끊겼다: 머리는 그대로 두고 방송 목록 자리에 안내와 다시 연결 -->
        <div role="alert">
          <EmptyState
            icon="alert"
            title="서버와 연결이 끊어졌습니다."
            description={retryError ?? closedReasonDetail(conn.closedReason[id] ?? "") ?? undefined}
          >
            {#snippet actions()}
              <button type="button" class="btn primary" disabled={retrying} aria-busy={retrying} onclick={reconnect}>
                <Icon name="refresh" size={15} />다시 연결
              </button>
            {/snippet}
          </EmptyState>
        </div>
      {/if}
    </div>
  {/if}
</div>

<RenameServerDialog bind:open={renameOpen} server={server ?? null} />
<RemoveServerDialog bind:open={removeOpen} server={server ?? null} ondone={() => (location.hash = "#/")} />

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
</style>
