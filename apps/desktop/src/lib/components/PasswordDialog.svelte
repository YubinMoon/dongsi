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
    onsubmit: (password: string, remember: boolean) => Promise<void>;
  } = $props();

  let password = $state("");
  let remember = $state(true);
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
      await onsubmit(password, remember);
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
  <label class="check">
    <input type="checkbox" bind:checked={remember} form="broadcast-password" />
    이 서버에서 비밀번호 기억하기
  </label>
  {#if error}
    <p class="error small" role="alert">{error}</p>
  {/if}

  {#snippet actions()}
    <button type="button" onclick={() => (open = false)}>취소</button>
    <button type="submit" form="broadcast-password" class="primary" disabled={busy}>확인</button>
  {/snippet}
</Modal>

<style>
  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.9rem;
  }

  .check input {
    width: auto;
  }
</style>
