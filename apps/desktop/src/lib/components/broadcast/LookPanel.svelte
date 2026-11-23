<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../Icon.svelte";
  import Panel from "./Panel.svelte";
  import SwitchSourceDialog from "./SwitchSourceDialog.svelte";
  import { api } from "$lib/api";
  import { canSwitchSource } from "$lib/broadcast";
  import type { BroadcastStatus } from "$lib/types";
  import { decodePreview, PREVIEW_IDLE_MS, PREVIEW_POLL_MS, previewRequestSize, type PreviewImage } from "$lib/livePreview";

  let { status, style }: { status: BroadcastStatus; style?: string } = $props();

  /** 화면 전환 대화 상자(D-086) */
  let switching = $state(false);

  let canvas = $state<HTMLCanvasElement | null>(null);
  /** 첫 그림을 받았다(그 전에는 검은 칸만) */
  let shown = $state(false);

  function draw(img: PreviewImage) {
    if (!canvas) return;
    if (canvas.width !== img.width || canvas.height !== img.height) {
      canvas.width = img.width;
      canvas.height = img.height;
    }
    canvas.getContext("2d")?.putImageData(new ImageData(img.rgba, img.width, img.height), 0, 0);
    shown = true;
  }

  // 0.1초마다 새 그림을 묻는다. 창이 가려졌으면 묻지 않는다(백엔드는 1초 동안 요청이 없으면 만들지 않는다).
  onMount(() => {
    let alive = true;
    let after = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      const started = performance.now();
      let wait = PREVIEW_POLL_MS;
      if (!canvas || document.hidden) {
        wait = PREVIEW_IDLE_MS;
      } else {
        try {
          const size = previewRequestSize(canvas.clientWidth, canvas.clientHeight, window.devicePixelRatio);
          const img = decodePreview(await api.broadcastPreview(after, size.width, size.height));
          if (img && alive) {
            after = img.seq;
            draw(img);
          }
        } catch {
          // 미리 보기가 없어도 방송에는 영향이 없다. 조금 쉬었다가 다시 묻는다.
          wait = PREVIEW_IDLE_MS;
        }
      }
      if (alive) timer = setTimeout(tick, Math.max(0, wait - (performance.now() - started)));
    };
    void tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  });
</script>

<!--
  송출 중인 화면만 보인다(D-066). 이 PC에서는 내 방송을 시청하지 않으므로(D-065) 서버를 거치지 않고
  인코더에 들어가는 프레임을 줄여 그린다. 소리는 없다.
-->
<Panel title="다른 사람에게 보이는 모습" {style}>
  {#snippet aside()}
    {#if canSwitchSource(status)}
      <button type="button" class="btn sm" onclick={() => (switching = true)}>
        <Icon name="monitor" size={14} />화면 전환
      </button>
    {/if}
  {/snippet}
  <div class="screen">
    <canvas bind:this={canvas} class:shown aria-label="송출 중인 화면"></canvas>
  </div>
</Panel>

<SwitchSourceDialog bind:open={switching} {status} />

<style>
  .screen {
    position: relative;
    aspect-ratio: 16 / 9;
    border-radius: var(--r-tile);
    overflow: hidden;
    background: #000;
  }

  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    visibility: hidden;
  }

  canvas.shown {
    visibility: visible;
  }
</style>
