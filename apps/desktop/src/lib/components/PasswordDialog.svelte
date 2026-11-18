<script lang="ts">
  import Modal from "./Modal.svelte";
  import { errorMessage } from "$lib/api";

  let {
    open = $bindable(false),
    broadcasterName,
    onsubmit,
  }: {
    open?: boolean;
    broadcasterName: string;
    onsubmit: (password: string) => Promise<void>;
  } = $props();

  let password = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);

  $effect(() => {
    if (!open) {
      password = "";
      error = null;
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!password) {
      error = "비밀번호를 입력해 주세요";
      return;
    }
    busy = true;
    try {
      await onsubmit(password);
      open = false;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<Modal bind:open title="비밀번호가 걸린 방송">
  <form id="broadcast-password" onsubmit={submit}>
    <label for="password-input">'{broadcasterName}' 방송의 비밀번호</label>
    <!-- svelte-ignore a11y_autofocus -->
    <input id="password-input" type="password" bind:value={password} autocomplete="off" autofocus />
  </form>
  <p class="muted small">
    서버 연결을 구현하기 전이라, 지금은 비밀번호가 맞는지 확인하지 않고 기억만 합니다.
  </p>
  {#if error}
    <p class="error small" role="alert">{error}</p>
  {/if}

  {#snippet actions()}
    <button type="button" onclick={() => (open = false)}>취소</button>
    <button type="submit" form="broadcast-password" class="primary" disabled={busy}>확인</button>
  {/snippet}
</Modal>
