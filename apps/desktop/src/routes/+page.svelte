<script lang="ts">
  import FirstRunBox from "$lib/components/home/FirstRunBox.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import LiveBroadcasts from "$lib/components/home/LiveBroadcasts.svelte";
  import { refreshServers, serverStore } from "$lib/stores/servers.svelte";

  // 홈에는 "방송 중" 목록만 둔다(D-073). 저장한 서버는 위쪽 바 탭에 모두 있다(이름 변경·삭제·다시 연결은 탭 메뉴).
  /** 목록을 읽었고 비어 있으면 처음 실행 상자를 맨 위에 보인다. */
  const firstRun = $derived(serverStore.loaded && serverStore.error === null && serverStore.list.length === 0);
</script>

<div class="page stack" class:first-run={firstRun}>
  {#if firstRun}
    <FirstRunBox />
  {/if}

  {#if serverStore.error}
    <div class="load-error" role="alert">
      <span>서버 목록을 읽지 못했습니다. {serverStore.error}</span>
      <button type="button" class="btn sm" onclick={refreshServers}>
        <Icon name="refresh" size={13} />다시 불러오기
      </button>
    </div>
  {/if}

  <LiveBroadcasts />
</div>

<style>
  .page {
    --gap: 32px;
  }

  /* b-first-run은 위 여백 32, 구획 간격 36 */
  .page.first-run {
    --gap: 36px;
    padding-top: 32px;
  }

  .load-error {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    font-size: 13px;
    color: var(--red-text);
  }
</style>
