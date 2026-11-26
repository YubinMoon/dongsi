<script lang="ts">
  import { AUDIO_BOOST_MAX, AUDIO_BOOST_MIN, AUDIO_BOOST_STEP, normalizeAudioBoost } from "$lib/broadcast";

  // 소리 증폭: 시청자에게 보내는 소리에 곱하는 배율(100~300%). 바를 놓거나 키보드로 바꾸면 적용한다.
  let {
    id,
    value = $bindable(AUDIO_BOOST_MIN),
    oncommit,
  }: {
    id: string;
    /** % */
    value?: number;
    /** 값을 확정했다(바를 놓음, 키보드로 바꿈). */
    oncommit?: (percent: number) => void;
  } = $props();

  const pos = $derived((value - AUDIO_BOOST_MIN) / (AUDIO_BOOST_MAX - AUDIO_BOOST_MIN));

  function slide(e: Event) {
    value = normalizeAudioBoost(Number((e.currentTarget as HTMLInputElement).value));
  }
</script>

<div class="boost">
  <div class="head">
    <label class="label" for={id}>소리 증폭</label>
    <span class="hint num value">{value}%</span>
  </div>
  <input
    {id}
    class="bar"
    type="range"
    min={AUDIO_BOOST_MIN}
    max={AUDIO_BOOST_MAX}
    step={AUDIO_BOOST_STEP}
    {value}
    style:--pos={pos}
    aria-valuetext="{value}%"
    oninput={slide}
    onchange={() => oncommit?.(value)}
  />
  <p class="hint">시청자에게 보내는 소리를 키웁니다. 넘치는 큰 소리는 자동으로 눌러 깨지지 않게 합니다.</p>
</div>

<style>
  .boost {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .value {
    margin-left: auto;
    color: var(--text);
  }

  .bar {
    /* 손잡이(16px) 가운데가 닿는 곳까지 채운다(비트레이트 바와 같은 모양). */
    --thumb: 16px;
    --at: calc(var(--thumb) / 2 + (100% - var(--thumb)) * var(--pos));
    -webkit-appearance: none;
    appearance: none;
    display: block;
    width: 100%;
    height: 22px;
    margin: 0;
    background: transparent;
    cursor: pointer;
  }

  .bar::-webkit-slider-runnable-track {
    height: 6px;
    border-radius: 3px;
    background: linear-gradient(to right, var(--text-2) 0 var(--at), var(--fill-active) var(--at) 100%);
  }

  .bar::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: var(--thumb);
    height: var(--thumb);
    margin-top: -5px;
    border-radius: 50%;
    background: var(--text);
    box-shadow:
      0 0 0 3px var(--s1),
      0 1px 4px rgba(0, 0, 0, 0.5);
  }

  .bar:focus-visible {
    outline: none;
  }

  .bar:focus-visible::-webkit-slider-thumb {
    outline: var(--focus-ring);
    outline-offset: 2px;
  }
</style>
