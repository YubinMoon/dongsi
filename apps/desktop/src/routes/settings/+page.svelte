<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import BroadcastTile from "$lib/components/BroadcastTile.svelte";
  import Panel from "$lib/components/broadcast/Panel.svelte";
  import { api, errorMessage } from "$lib/api";
  import { toast } from "$lib/toast.svelte";
  import type { Settings } from "$lib/types";

  const uid = $props.id();

  let settings = $state<Settings | null>(null);
  let displayName = $state("");
  let error = $state<string | null>(null);
  let saving = $state(false);
  let version = $state("");

  onMount(async () => {
    try {
      settings = await api.getSettings();
      displayName = settings.displayName;
    } catch (e) {
      error = errorMessage(e);
    }
    try {
      version = await getVersion();
    } catch {
      // Tauri 밖에서는 버전을 읽지 못한다.
    }
  });

  const changed = $derived(settings !== null && displayName.trim() !== settings.displayName);
  /** 미리 보기에 쓰는 이름: 입력 중인 이름, 비어 있으면 저장된 이름 */
  const previewName = $derived(displayName.trim() || settings?.displayName || "");

  async function save(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    try {
      settings = await api.setDisplayName(displayName);
      displayName = settings.displayName;
      error = null;
      toast("표시 이름을 저장했습니다", "success");
    } catch (e) {
      error = errorMessage(e);
    } finally {
      saving = false;
    }
  }
</script>

<div class="page narrow stack" style="--gap: 16px">
  <h1 class="page-title">설정</h1>

  {#if settings}
    <Panel title="프로필" size="lg">
      <div class="profile">
        <form onsubmit={save}>
          <label class="label" for="{uid}-name">표시 이름</label>
          <div class="row">
            <input id="{uid}-name" bind:value={displayName} maxlength="32" autocomplete="off" />
            <button type="submit" class="btn save" disabled={!changed || saving}>저장</button>
          </div>
          <p class="hint">방송 목록에서 다른 사람에게 보이는 이름입니다.</p>
          {#if error}<p class="error" role="alert">{error}</p>{/if}
        </form>

        <figure>
          <figcaption>다른 사람에게는 이렇게 보입니다</figcaption>
          <BroadcastTile
            seed={settings.installId}
            name={previewName}
            elapsed="12분"
            viewers={2}
            meta="1080p · 60fps · H.264"
            mine
          />
        </figure>
      </div>
    </Panel>

    <Panel title="이 앱" size="lg">
      <dl class="about">
        <dt>설치 ID</dt>
        <dd class="stacked">
          <span class="mono install-id">{settings.installId}</span>
          <span class="desc">
            이 PC의 앱을 구분하는 ID입니다. 방송 초대 코드에 들어가서, 어느 방송자의 비밀번호인지 구분하는 데 쓰입니다.
          </span>
        </dd>
        <dt>버전</dt>
        <dd class="num">{version}</dd>
      </dl>
    </Panel>
  {:else if error}
    <p class="error" role="alert">{error}</p>
  {:else}
    <p class="loading">불러오는 중…</p>
  {/if}
</div>

<style>
  .narrow {
    max-width: 820px;
    padding-top: 28px;
  }

  .profile {
    display: flex;
    gap: 24px;
    flex-wrap: wrap;
    align-items: flex-start;
  }

  form {
    flex: 1 1 300px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
  }

  .row {
    display: flex;
    gap: 8px;
  }

  .row input {
    flex: 1;
    min-width: 0;
  }

  .save {
    height: 36px;
    font-weight: 650;
  }

  form .hint {
    font-size: 13px;
  }

  .error {
    font-size: 13px;
    color: var(--red-text);
  }

  figure {
    flex: 0 1 240px;
    min-width: 200px;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  figcaption {
    font-size: 12px;
    color: var(--text-3);
  }

  .about {
    display: grid;
    grid-template-columns: 120px minmax(0, 1fr);
    gap: 14px 16px;
    margin: 0;
    font-size: 14px;
  }

  dt {
    color: var(--text-2);
    font-weight: 600;
    font-size: 13px;
  }

  dd {
    margin: 0;
  }

  .stacked {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .install-id {
    font-size: 13px;
    overflow-wrap: anywhere;
  }

  .desc {
    font-size: 13px;
    color: var(--text-3);
    text-wrap: pretty;
  }

  .loading {
    color: var(--text-2);
  }
</style>
