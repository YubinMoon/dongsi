<script lang="ts">
  import { onMount } from "svelte";
  import Panel from "./Panel.svelte";
  import BitrateInput from "./BitrateInput.svelte";
  import { Capabilities } from "./capabilities.svelte";
  import QualityFields from "./QualityFields.svelte";
  import { api, errorMessage } from "$lib/api";
  import { codecOptions, normalizeBitrate, presetBitrate, presetIdFor, settingsFor } from "$lib/broadcast";
  import { setBroadcastStatus } from "$lib/stores/broadcast.svelte";
  import { toast } from "$lib/toast.svelte";
  import type { BroadcastStatus, VideoCodec } from "$lib/types";

  let { status, style }: { status: BroadcastStatus; style?: string } = $props();

  const uid = $props.id();
  const caps = new Capabilities();

  let livePreset = $state("");
  let liveCodec = $state<VideoCodec>("h264");
  let liveBitrate = $state<number | null>(0);
  let applying = $state(false);

  const codecOpts = $derived(codecOptions(caps.encoders, caps.receivable));

  // 프리셋·코덱을 바꾸면 비트레이트를 그 기본값으로(0.1.0 onLivePreset/onLiveCodec).
  function followBitrate(presetId: string, codec: VideoCodec) {
    const b = presetBitrate(caps.presets.find((p) => p.id === presetId) ?? null, codec);
    if (b !== null) liveBitrate = b;
  }

  async function apply() {
    const p = caps.presets.find((x) => x.id === livePreset) ?? null;
    applying = true;
    try {
      const next = await api.updateBroadcast(
        settingsFor(p, liveCodec, normalizeBitrate(liveBitrate, status.settings.bitrateKbps)),
      );
      setBroadcastStatus(next);
      toast("방송 설정을 바꿨습니다", "success");
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      applying = false;
    }
  }

  onMount(async () => {
    caps.readReceivable();
    void caps.loadEncoders();
    // 지금 방송 설정으로 처음 채운다. 이 뒤로는 1초마다 오는 상태가 입력을 덮어쓰지 않는다.
    liveCodec = status.settings.codec;
    liveBitrate = status.settings.bitrateKbps;
    try {
      await caps.loadPresets();
      livePreset = presetIdFor(caps.presets, status.settings);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  });
</script>

<Panel title="화질 변경" {style}>
  <QualityFields
    presets={caps.presets}
    presetId={livePreset}
    codec={liveCodec}
    {codecOpts}
    onpreset={(id) => {
      livePreset = id;
      followBitrate(id, liveCodec);
    }}
    oncodec={(c) => {
      liveCodec = c as VideoCodec;
      followBitrate(livePreset, liveCodec);
    }}
  />

  <div class="group">
    <label class="label" for="{uid}-bitrate">비트레이트</label>
    <div class="row">
      <span class="field"><BitrateInput id="{uid}-bitrate" bind:value={liveBitrate} /></span>
      <button type="button" class="btn primary apply" onclick={apply} disabled={applying}>
        {applying ? "적용하는 중…" : "적용"}
      </button>
    </div>
  </div>

  <p class="hint">방송을 끊지 않고 바꿉니다. 코덱이나 해상도를 바꾸면 잠깐 키프레임을 다시 보냅니다.</p>
</Panel>

<style>
  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .row {
    display: flex;
    gap: 8px;
  }

  .field {
    flex: 1;
    min-width: 0;
    max-width: 200px;
  }

  .apply {
    height: 36px;
    padding: 0 16px;
  }
</style>
