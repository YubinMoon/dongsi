<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../Icon.svelte";
  import LiveQualityPanel from "./LiveQualityPanel.svelte";
  import LookPanel from "./LookPanel.svelte";
  import SharePanel from "./SharePanel.svelte";
  import StatusPanel from "./StatusPanel.svelte";
  import { api, errorMessage } from "$lib/api";
  import { formatClock, secondsSince } from "$lib/format";
  import { onAir, setBroadcastStatus } from "$lib/stores/broadcast.svelte";
  import { toast } from "$lib/toast.svelte";
  import type { BroadcastStatus } from "$lib/types";

  let { status }: { status: BroadcastStatus } = $props();

  let busy = $state(false);
  let displayName = $state<string | null>(null);

  async function stop() {
    busy = true;
    try {
      await api.stopBroadcast();
      setBroadcastStatus(null);
      toast("방송을 끝냈습니다");
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      busy = false;
    }
  }

  onMount(async () => {
    try {
      displayName = (await api.getSettings()).displayName;
    } catch {
      // 이름을 못 읽으면 서버 이름으로 대신 보인다.
    }
  });
</script>

<div class="page stack" style="--gap: 16px">
  <div class="head">
    <h1><span class="dot" aria-hidden="true"></span>방송 중</h1>
    <span class="clock mono">{formatClock(secondsSince(status.startedAt, onAir.now))}</span>
    <span class="viewers"><Icon name="eye" size={16} />시청자 {status.viewers}명</span>
    {#if status.locked}
      <span class="badge lock"><Icon name="lock" size={12} />비밀번호 걸림</span>
    {/if}
    <span class="spacer"></span>
    <!-- 내 방송 미리 보기(시청 화면)는 두지 않는다: 이 PC에서 소리를 틀면 에코가 쌓인다(D-065). -->
    <div class="actions">
      <button type="button" class="btn danger-soft" onclick={stop} disabled={busy}>
        <Icon name="square" size={15} />방송 끝내기
      </button>
    </div>
  </div>

  <div class="row">
    <LookPanel {status} name={displayName ?? status.serverName} now={onAir.now} style="flex: 1 1 340px" />
    <SharePanel {status} style="flex: 1.5 1 420px" />
  </div>
  <div class="row">
    <LiveQualityPanel {status} style="flex: 1 1 420px" />
    <StatusPanel {status} style="flex: 1 1 420px" />
  </div>
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 4px;
  }

  h1 {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }

  .dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--red-dot);
    box-shadow: 0 0 0 4px rgba(229, 56, 59, 0.22);
  }

  .clock {
    font-size: 20px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }

  .viewers {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
  }

  .lock {
    height: 24px;
    padding: 0 9px;
    gap: 5px;
    background: rgba(255, 255, 255, 0.08);
    font-size: 12px;
    color: var(--text);
  }

  .spacer {
    flex: 1;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
  }
</style>
