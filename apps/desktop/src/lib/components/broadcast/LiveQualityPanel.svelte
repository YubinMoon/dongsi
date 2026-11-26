<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../Icon.svelte";
  import Spinner from "../Spinner.svelte";
  import Panel from "./Panel.svelte";
  import BitrateField from "./BitrateField.svelte";
  import QualityFields from "./QualityFields.svelte";
  import { api, errorMessage } from "$lib/api";
  import { normalizeKbps } from "$lib/bitrate";
  import { bitrateZones } from "$lib/bitrateZones";
  import {
    DEFAULT_QUALITY,
    LatestOnly,
    liveSource,
    networkLimitHint,
    qualityTarget,
    settingsFor,
    type QualityChoice,
  } from "$lib/broadcast";
  import { onAir, setBroadcastStatus } from "$lib/stores/broadcast.svelte";
  import { toast } from "$lib/toast.svelte";
  import type { BroadcastStatus, VideoSettings } from "$lib/types";

  let { status, style }: { status: BroadcastStatus; style?: string } = $props();

  const uid = $props.id();

  // 고른 값. 처음에 지금 방송 설정으로 채우고, 이 뒤로는 1초마다 오는 상태가 덮어쓰지 않는다.
  let quality = $state<QualityChoice>({ ...DEFAULT_QUALITY });
  let bitrate = $state<number | null>(null);
  /** 조용한 적용 표시: 적용하는 중 / 적용됨(잠깐). 실패는 알림으로. */
  let phase = $state<"idle" | "applying" | "done">("idle");
  let doneTimer: ReturnType<typeof setTimeout> | undefined;

  // 방송 중에는 모니터 주사율을 모른다(fps는 설정 그대로, D-107). 그래서 주사율 안내는 방송 설정 화면에만 있다.
  const source = $derived(liveSource(status));
  const target = $derived(qualityTarget(quality, source));
  const zones = $derived(bitrateZones(target));
  const netHint = $derived(networkLimitHint(status.publish));

  function fill(s: VideoSettings) {
    quality = { width: s.width, height: s.height, fps: s.fps };
    bitrate = s.bitrateKbps;
  }

  // 바뀔 때마다 바로 보낸다(D-074). 보내는 중에 또 바뀌면 마지막 값만 이어서 보낸다.
  const sender = new LatestOnly<VideoSettings>(
    async (settings) => {
      clearTimeout(doneTimer);
      phase = "applying";
      try {
        setBroadcastStatus(await api.updateBroadcast(settings));
        return true;
      } catch (e) {
        toast(`방송 설정을 바꾸지 못했습니다: ${errorMessage(e)}`, "error");
        // 화면을 지금 방송 설정으로 되돌린다.
        fill((onAir.status ?? status).settings);
        return false;
      }
    },
    (ok) => {
      phase = ok ? "done" : "idle";
      if (ok) doneTimer = setTimeout(() => (phase = "idle"), 2000);
    },
  );

  function apply() {
    void sender.push(settingsFor(quality, normalizeKbps(bitrate, zones.recommended)));
  }

  /** 해상도·주사율을 바꾸면 비트레이트는 새 조합의 "적당" 기본값으로 같이 바꿔 바로 적용한다. */
  function pick(change: Partial<QualityChoice>) {
    quality = { ...quality, ...change };
    bitrate = bitrateZones(qualityTarget(quality, source)).recommended;
    apply();
  }

  onMount(() => {
    fill(status.settings);
    sender.mark(status.settings);
    return () => clearTimeout(doneTimer);
  });
</script>

<Panel title="화질 변경" {style}>
  {#snippet aside()}
    <span class="state" role="status" aria-live="polite">
      {#if phase === "applying"}
        <Spinner size={12} />적용하는 중…
      {:else if phase === "done"}
        <span class="ok"><Icon name="check" size={13} />적용됨</span>
      {/if}
    </span>
  {/snippet}

  <QualityFields
    width={quality.width}
    height={quality.height}
    fps={quality.fps}
    onresolution={(width, height) => pick({ width, height })}
    onfps={(fps) => pick({ fps })}
  />

  <BitrateField
    id="{uid}-bitrate"
    bind:value={bitrate}
    {zones}
    hosting={status.hosting}
    viewers={status.viewers}
    oncommit={apply}
  />

  {#if netHint}
    <p class="hint net" role="status">{netHint}</p>
  {/if}

  <p class="hint">바꾸면 바로 적용됩니다. 방송은 끊기지 않고, 해상도를 바꾸면 잠깐 키프레임을 다시 보냅니다.</p>
</Panel>

<style>
  .state {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 20px;
    font-size: 12px;
    color: var(--text-3);
  }

  .ok {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--green-text);
  }

  .net {
    color: var(--amber-text);
  }
</style>
