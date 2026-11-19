<script lang="ts">
  import Dialog from "./Dialog.svelte";
  import { api, errorMessage } from "$lib/api";
  import { refreshServers } from "$lib/stores/servers.svelte";
  import { toast } from "$lib/toast.svelte";
  import type { AddResult, ParsedInput } from "$lib/types";

  /**
   * 서버 추가 대화 상자. 레이아웃이 `ui.addServerOpen`에 묶어 전역으로 하나 둔다.
   * `onadded`를 주면 추가 뒤 처리(알림·목록 갱신)를 그쪽에 맡기고, 없으면 여기서 알림과 `refreshServers()`를 한다.
   */
  let {
    open = $bindable(false),
    onadded,
  }: {
    open?: boolean;
    onadded?: (result: AddResult) => void;
  } = $props();

  async function defaultAdded(result: AddResult) {
    const name = result.server.name;
    if (result.outcome === "added") toast(`'${name}' 서버를 추가했습니다`, "success");
    else if (result.outcome === "updated") toast(`'${name}' 서버의 정보를 갱신했습니다`, "success");
    else toast(`'${name}'(으)로 이미 저장된 서버입니다`, "info");
    await refreshServers();
  }

  let text = $state("");
  let name = $state("");
  let parsed = $state<ParsedInput | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let requestSeq = 0;

  // 입력이 멈추면 Rust 쪽에서 해석해 미리보기를 보여 준다.
  $effect(() => {
    const value = text.trim();
    parsed = null;
    error = null;
    if (!value) return;
    const seq = ++requestSeq;
    const timer = setTimeout(async () => {
      try {
        const result = await api.parseServerInput(value);
        if (seq === requestSeq) parsed = result;
      } catch (e) {
        if (seq === requestSeq) error = errorMessage(e);
      }
    }, 200);
    return () => clearTimeout(timer);
  });

  // 닫힐 때 입력을 비운다.
  $effect(() => {
    if (!open) {
      text = "";
      name = "";
    }
  });

  const canSubmit = $derived(
    parsed !== null && !busy && !(parsed.kind === "address" && parsed.existing !== null),
  );
  const submitLabel = $derived(parsed?.kind === "invite" && parsed.existing ? "갱신" : "추가");

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!canSubmit || !parsed) return;
    busy = true;
    try {
      const customName = parsed.kind === "address" && name.trim() ? name.trim() : null;
      const result = await api.addServer(text.trim(), customName);
      open = false;
      if (onadded) onadded(result);
      else await defaultAdded(result);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  async function fillSample() {
    try {
      text = await api.devSampleInvite();
    } catch (e) {
      error = errorMessage(e);
    }
  }
</script>

<Dialog bind:open title="서버 추가" width={480}>
  <form id="add-server" onsubmit={submit}>
    <label for="server-input">주소 또는 초대 코드</label>
    <textarea
      id="server-input"
      bind:value={text}
      rows="3"
      spellcheck="false"
      placeholder={"192.168.0.10, example.com:7884 같은 주소나\ndongsi1: 로 시작하는 초대 코드를 붙여 넣으세요"}
    ></textarea>
    {#if import.meta.env.DEV}
      <button type="button" class="link small" onclick={fillSample}>예시 초대 코드 넣기 (개발용)</button>
    {/if}
  </form>

  {#if error}
    <p class="error small" role="alert">{error}</p>
  {:else if parsed?.kind === "address"}
    <div class="preview">
      <dl>
        <dt>종류</dt>
        <dd>주소로 추가</dd>
        <dt>주소</dt>
        <dd class="mono">{parsed.addr}</dd>
      </dl>
      {#if parsed.existing}
        <p class="warning small">이미 '{parsed.existing.name}'(으)로 저장된 주소입니다.</p>
      {:else}
        <p class="muted small">인증서 지문은 처음 접속할 때 확인하고 기억합니다.</p>
        <div>
          <label for="server-name">이름 (선택)</label>
          <input id="server-name" form="add-server" bind:value={name} placeholder={parsed.addr} maxlength="64" />
        </div>
      {/if}
    </div>
  {:else if parsed?.kind === "invite"}
    <div class="preview">
      <dl>
        <dt>종류</dt>
        <dd>
          {parsed.broadcast ? "방송 초대 코드" : "서버 초대 코드"}
          {#if parsed.hasPassword}<span class="badge">비밀번호 포함</span>{/if}
        </dd>
        <dt>이름</dt>
        <dd>{parsed.name ?? "(없음)"}</dd>
        <dt>주소 후보</dt>
        <dd class="mono">{parsed.addrs.join(", ")}</dd>
        <dt>인증서 지문</dt>
        <dd class="mono" title={parsed.fingerprint.full}>{parsed.fingerprint.short}…</dd>
      </dl>
      {#if parsed.existing}
        <p class="muted small">이미 저장된 서버 '{parsed.existing.name}'의 주소를 갱신합니다.</p>
      {/if}
    </div>
  {/if}

  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (open = false)}>취소</button>
    <button type="submit" form="add-server" class="btn primary" disabled={!canSubmit}>
      {submitLabel}
    </button>
  {/snippet}
</Dialog>

<style>
  form {
    display: grid;
    gap: 0.4rem;
    justify-items: start;
  }

  .preview {
    display: grid;
    gap: 0.6rem;
  }

  dl {
    margin: 0;
    display: grid;
    grid-template-columns: 6.5rem 1fr;
    gap: 0.35rem 0.8rem;
    font-size: 0.9rem;
  }

  dt {
    color: var(--text-muted);
  }

  dd {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
    word-break: break-all;
  }

  p {
    margin: 0;
  }
</style>
