<script lang="ts">
  import { initial } from "$lib/format";
  import { hueFor, tileVars } from "$lib/hue";
  import { passwordLabel, PASSWORD_REQUIRED_MESSAGE } from "$lib/watch";
  import Icon from "../Icon.svelte";

  let {
    name,
    broadcasterId,
    subtitle,
    error = null,
    busy = false,
    onsubmit,
  }: {
    name: string;
    /** 아바타 색조를 뽑는 ID */
    broadcasterId: string;
    /** 제목 아래 줄: "도윤 · 1440p · 30fps" */
    subtitle: string;
    /** 입력란 아래 오류 줄(틀린 비밀번호 등) */
    error?: string | null;
    /** 확인을 보내는 중 */
    busy?: boolean;
    onsubmit: (password: string, remember: boolean) => void;
  } = $props();

  const uid = $props.id();
  let password = $state("");
  let remember = $state(true);
  let localError = $state<string | null>(null);
  let input: HTMLInputElement | undefined = $state();

  const shownError = $derived(localError ?? error);

  // 처음 보일 때와, 보낸 뒤 틀려서 돌아왔을 때 입력란에 초점을 둔다(입력한 글자는 그대로 둔다).
  $effect(() => {
    if (!busy) input?.focus();
  });

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    if (!password) {
      localError = PASSWORD_REQUIRED_MESSAGE;
      input?.focus();
      return;
    }
    localError = null;
    onsubmit(password, remember);
  }
</script>

<form class="card" style={tileVars(hueFor(broadcasterId))} onsubmit={submit}>
  <div class="head">
    <span class="avatar" aria-hidden="true">
      {initial(name)}
      <span class="lock"><Icon name="lock" size={12} /></span>
    </span>
    <div class="titles">
      <h2>비밀번호가 걸린 방송</h2>
      <span class="sub">{subtitle}</span>
    </div>
  </div>

  <div class="field">
    <label class="label" for="{uid}-pw">{passwordLabel(name)}</label>
    <div class="row">
      <input
        id="{uid}-pw"
        class="lg"
        type="password"
        bind:this={input}
        bind:value={password}
        autocomplete="off"
        aria-invalid={shownError ? "true" : undefined}
        aria-describedby={shownError ? `${uid}-err` : undefined}
        oninput={() => (localError = null)}
      />
      <button type="submit" class="btn primary submit" disabled={busy} aria-busy={busy}>
        {#if busy}<span class="spin" aria-hidden="true"></span><span class="sr-only">확인 중</span>{:else}확인{/if}
      </button>
    </div>
    {#if shownError}<p id="{uid}-err" class="err" role="alert">{shownError}</p>{/if}
  </div>

  <label class="check">
    <input type="checkbox" bind:checked={remember} />
    이 서버에서 비밀번호 기억하기
  </label>
</form>

<style>
  .card {
    width: 100%;
    max-width: 400px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    margin: 0;
    padding: 24px;
    border-radius: var(--r-empty);
    background: var(--s1);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 24px 64px rgba(0, 0, 0, 0.5);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .avatar {
    position: relative;
    flex: none;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: oklch(0.72 0.11 var(--h));
    color: oklch(0.22 0.04 var(--h));
    font-size: 19px;
    font-weight: 700;
  }

  .lock {
    position: absolute;
    right: -4px;
    bottom: -4px;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--s3);
    box-shadow: 0 0 0 3px var(--s1);
    color: #ffffff;
  }

  .titles {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  h2 {
    font-size: 16px;
    font-weight: 650;
    letter-spacing: -0.015em;
  }

  .sub {
    font-size: 13px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .row {
    display: flex;
    gap: 8px;
  }

  .row input {
    flex: 1;
    min-width: 0;
  }

  .btn.primary.submit {
    height: 38px;
    padding: 0 16px;
    min-width: 62px;
  }

  .spin {
    width: 14px;
    height: 14px;
    box-sizing: border-box;
    border-radius: 50%;
    border: 2px solid rgba(13, 14, 17, 0.25);
    border-top-color: var(--bg);
    animation: spin 0.9s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none;
    }
  }

  .err {
    font-size: 13px;
    color: var(--red-text);
  }

  .check {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    align-self: flex-start;
    font-size: 13px;
    color: var(--text-2);
    cursor: pointer;
  }

  .check input {
    width: 16px;
    height: 16px;
    margin: 0;
  }
</style>
