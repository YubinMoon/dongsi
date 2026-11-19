<script lang="ts">
  import Icon from "../Icon.svelte";
  import Notice from "./Notice.svelte";
  import Panel from "./Panel.svelte";
  import { HOSTING_ICON, shareLine, shareNotices } from "$lib/broadcast";
  import { toast } from "$lib/toast.svelte";
  import type { BroadcastStatus } from "$lib/types";

  let { status, style }: { status: BroadcastStatus; style?: string } = $props();

  const uid = $props.id();
  const notices = $derived(shareNotices(status));

  async function copy(text: string, what: string) {
    try {
      await navigator.clipboard.writeText(text);
      toast(`${what}를 복사했습니다`, "success");
    } catch {
      toast("복사하지 못했습니다", "error");
    }
  }
</script>

<Panel title="공유" {style}>
  <p class="where">
    <Icon name={HOSTING_ICON[status.hosting]} size={15} />{shareLine(status)}
  </p>

  {#each notices as n (n.id)}
    <Notice tone={n.tone} icon={n.tone === "ok" ? "check" : "alert"} boxed={n.tone === "warn"}>
      {n.text}{#if n.code}<span class="mono code">{n.code}</span>{/if}
    </Notice>
  {/each}

  <div class="invite">
    <label class="label" for="{uid}-invite">초대 코드</label>
    <div class="copy-row">
      <input id="{uid}-invite" class="mono" readonly value={status.invite} title={status.invite} />
      <button type="button" class="btn primary copy" onclick={() => copy(status.invite, "초대 코드")}>
        <Icon name="copy" size={15} />복사
      </button>
    </div>
    {#if status.inviteWithoutPassword}
      {@const plain = status.inviteWithoutPassword}
      <button type="button" class="linkbtn" onclick={() => copy(plain, "비밀번호 없는 초대 코드")}>
        비밀번호 없이 복사
      </button>
    {/if}
  </div>

  <p class="hint">
    친구가 dongsi의 "서버 추가"에 붙여 넣으면 이 방송을 볼 수 있습니다. 같은 LAN·Tailscale에서는 자동으로 보입니다.
  </p>
</Panel>

<style>
  .where {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text-2);
  }

  .code {
    font-size: 12px;
  }

  .invite {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .copy-row {
    display: flex;
    gap: 8px;
  }

  .copy-row input {
    flex: 1;
    min-width: 0;
    font-size: 12px;
    text-overflow: ellipsis;
  }

  .copy {
    height: 36px;
  }

  .linkbtn {
    align-self: flex-start;
    font-size: 12px;
    color: var(--text-2);
    text-decoration: underline;
    text-underline-offset: 3px;
  }

  .linkbtn:hover {
    color: var(--text);
  }

  .hint {
    text-wrap: pretty;
  }
</style>
