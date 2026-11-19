<script lang="ts">
  import Icon from "../Icon.svelte";
  import { openAddServer } from "$lib/stores/ui.svelte";
</script>

<!-- 서버가 하나도 없을 때 홈 맨 위에 보이는 안내(b-first-run) -->
<section class="first-run" aria-labelledby="first-run-title">
  <span class="art" aria-hidden="true">
    <span class="back"></span>
    <span class="front"><span class="dot"></span></span>
    <span class="live">LIVE</span>
  </span>
  <div class="text">
    <h1 id="first-run-title">아직 추가한 서버가 없습니다</h1>
    <p>서버 주소를 입력하거나, 친구에게 받은 초대 코드를 붙여 넣어 추가하세요.</p>
    <div class="actions">
      <button type="button" class="btn primary" onclick={openAddServer}>
        <Icon name="plus" size={15} />서버 추가
      </button>
      <a class="btn" href="#/broadcast">
        서버 없이 내 PC에서 방송하기<Icon name="chevron-right" size={15} />
      </a>
    </div>
  </div>
</section>

<style>
  .first-run {
    display: flex;
    align-items: center;
    gap: 28px;
    flex-wrap: wrap;
    padding: 28px;
    border-radius: var(--r-empty);
    background: var(--s1);
    box-shadow: inset 0 0 0 1px var(--line);
  }

  .art {
    flex: none;
    position: relative;
    width: 132px;
    height: 84px;
  }

  .back,
  .front {
    position: absolute;
    border-radius: 8px;
    background-image: radial-gradient(rgba(255, 255, 255, 0.08) 1px, transparent 1.3px);
    background-size: 14px 14px;
  }

  .back {
    left: 0;
    top: 14px;
    width: 96px;
    height: 54px;
    background-color: oklch(0.25 0.035 250);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  .front {
    left: 30px;
    top: 0;
    width: 102px;
    height: 58px;
    display: grid;
    place-items: center;
    background-color: oklch(0.25 0.035 25);
    box-shadow: 0 0 0 3px var(--s1), inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  .dot {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: oklch(0.72 0.11 25);
  }

  .live {
    position: absolute;
    left: 38px;
    top: 8px;
    display: inline-flex;
    align-items: center;
    height: 14px;
    padding: 0 5px;
    border-radius: 4px;
    background: var(--red);
    color: #ffffff;
    font-size: 8px;
    font-weight: 700;
    letter-spacing: 0.05em;
  }

  .text {
    flex: 1 1 360px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 650;
    letter-spacing: -0.02em;
    line-height: 1.3;
  }

  p {
    font-size: 14px;
    color: var(--text-2);
    text-wrap: pretty;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 10px;
  }
</style>
