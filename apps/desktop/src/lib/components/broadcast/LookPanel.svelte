<script lang="ts">
  import BroadcastTile from "../BroadcastTile.svelte";
  import Panel from "./Panel.svelte";
  import { previewMeta } from "$lib/broadcast";
  import { formatAge, secondsSince } from "$lib/format";
  import type { BroadcastStatus } from "$lib/types";

  let {
    status,
    name,
    now,
    style,
  }: {
    status: BroadcastStatus;
    /** 내 표시 이름 */
    name: string;
    /** 지금 시각(ms) */
    now: number;
    style?: string;
  } = $props();
</script>

<Panel title="다른 사람에게 보이는 모습" {style}>
  <BroadcastTile
    seed={status.broadcasterId}
    {name}
    href="#/watch/{status.key}/{status.broadcasterId}"
    elapsed={formatAge(secondsSince(status.startedAt, now))}
    locked={status.locked}
    viewers={status.viewers}
    meta={previewMeta(status)}
    mine
  />
</Panel>
