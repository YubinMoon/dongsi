<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { api, errorMessage } from "$lib/api";
  import { toast } from "$lib/toast.svelte";
  import type { Settings } from "$lib/types";

  let settings = $state<Settings | null>(null);
  let displayName = $state("");
  let error = $state<string | null>(null);
  let saving = $state(false);
  let version = $state("");

  onMount(async () => {
    try {
      settings = await api.getSettings();
      displayName = settings.displayName;
      version = await getVersion();
    } catch (e) {
      error = errorMessage(e);
    }
  });

  const changed = $derived(settings !== null && displayName.trim() !== settings.displayName);

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

<h1>설정</h1>

{#if settings}
  <section class="card">
    <form onsubmit={save}>
      <label for="display-name">표시 이름</label>
      <div class="row">
        <input id="display-name" bind:value={displayName} maxlength="32" />
        <button type="submit" class="primary" disabled={!changed || saving}>저장</button>
      </div>
      <p class="muted small">방송 목록에서 다른 사람에게 보이는 이름입니다.</p>
      {#if error}<p class="error small" role="alert">{error}</p>{/if}
    </form>
  </section>

  <section class="card">
    <span class="label">설치 ID</span>
    <p class="mono">{settings.installId}</p>
    <p class="muted small">
      이 PC의 앱을 구분하는 ID입니다. 방송 초대 코드에 들어가서, 어느 방송자의 비밀번호인지
      구분하는 데 쓰입니다.
    </p>
  </section>

  <section class="card">
    <span class="label">버전</span>
    <p>{version}</p>
  </section>
{:else if error}
  <p class="error" role="alert">{error}</p>
{:else}
  <p class="muted">불러오는 중…</p>
{/if}

<style>
  h1 {
    margin-bottom: 1rem;
  }

  section {
    padding: 1rem 1.1rem;
    max-width: 560px;
  }

  section + section {
    margin-top: 0.8rem;
  }

  p {
    margin: 0.3rem 0 0;
  }

  .label {
    font-size: 0.85rem;
    color: var(--text-muted);
  }

  .row {
    display: flex;
    gap: 0.5rem;
  }

  .row input {
    flex: 1;
  }
</style>
