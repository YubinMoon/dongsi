<script lang="ts">
  import { tick } from "svelte";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  export interface MenuItem {
    label: string;
    icon?: IconName;
    danger?: boolean;
    onselect: () => void;
  }

  let {
    label,
    items,
    size = "sm",
    align = "end",
  }: {
    /** ⋯ 단추의 aria-label. 예: "'우리집 서버' 서버 메뉴" */
    label: string;
    items: MenuItem[];
    /** xs: 위쪽 바 탭 안(24×24), sm: 카드 안(32×32) */
    size?: "xs" | "sm";
    /** 펼친 메뉴를 ⋯ 단추의 어느 쪽에 맞출지. end: 오른쪽 끝(기본), start: 왼쪽 끝 */
    align?: "start" | "end";
  } = $props();

  /** 창 가장자리와 메뉴 사이의 최소 여백, ⋯ 단추와 메뉴 사이 */
  const MARGIN = 8;
  const GAP = 6;

  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  let trigger: HTMLButtonElement | undefined = $state();
  let menu: HTMLElement | undefined = $state();
  /** 메뉴의 창 기준 위치. 재기 전에는 null(보이지 않게 둔다). */
  let pos = $state<{ left: number; top: number } | null>(null);
  /** 오른쪽 클릭으로 열었을 때 그 자리 */
  let point: { x: number; y: number } | null = null;
  const uid = $props.id();

  function itemButtons(): HTMLButtonElement[] {
    return menu ? Array.from(menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')) : [];
  }

  /**
   * 메뉴는 창 기준(position: fixed)으로 둔다. 위쪽 바 탭처럼 가로 스크롤 상자(overflow) 안에 있어도 잘리지 않게 한다.
   * 창 밖으로 나가면 안쪽으로 당기고, 아래가 모자라면 위로 편다.
   */
  function place() {
    if (!menu || !trigger) return;
    const w = menu.offsetWidth;
    const h = menu.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left: number;
    let top: number;
    if (point) {
      left = point.x;
      top = point.y + h > vh - MARGIN ? point.y - h : point.y;
    } else {
      const r = trigger.getBoundingClientRect();
      left = align === "start" ? r.left : r.right - w;
      top = r.bottom + GAP;
      if (top + h > vh - MARGIN && r.top - GAP - h >= MARGIN) top = r.top - GAP - h;
    }
    left = Math.max(MARGIN, Math.min(left, vw - w - MARGIN));
    top = Math.max(MARGIN, top);
    pos = { left, top };
  }

  async function show(at: { x: number; y: number } | null = null) {
    point = at;
    pos = null;
    open = true;
    await tick();
    place();
    itemButtons()[0]?.focus({ preventScroll: true });
  }

  function hide(returnFocus = true) {
    if (!open) return;
    open = false;
    pos = null;
    if (returnFocus) trigger?.focus({ preventScroll: true });
  }

  function toggle() {
    if (open) hide(false);
    else void show();
  }

  /** 오른쪽 클릭 자리(창 기준 좌표)에 연다. 좌표가 없으면(키보드의 메뉴 키) ⋯ 단추 아래에 연다. */
  export function openAt(x: number, y: number) {
    void show(x > 0 || y > 0 ? { x, y } : null);
  }

  function select(item: MenuItem) {
    hide();
    item.onselect();
  }

  function onTriggerKeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!open) void show();
    }
  }

  function onMenuKeydown(e: KeyboardEvent) {
    const buttons = itemButtons();
    const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        buttons[(i + 1) % buttons.length]?.focus();
        break;
      case "ArrowUp":
        e.preventDefault();
        buttons[(i - 1 + buttons.length) % buttons.length]?.focus();
        break;
      case "Home":
        e.preventDefault();
        buttons[0]?.focus();
        break;
      case "End":
        e.preventDefault();
        buttons[buttons.length - 1]?.focus();
        break;
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        hide();
        break;
      case "Tab":
        hide(false);
        break;
    }
  }

  // 바깥을 누르면 닫는다. 창 기준 위치라서 스크롤하거나 창 크기가 바뀌면 ⋯ 단추를 따라 자리를 다시 잡는다
  // (닫으면 탭 줄을 스크롤해 ⋯ 단추를 보이게 한 직후 늦게 오는 scroll 이벤트가 막 연 메뉴를 닫는다).
  $effect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (root && !root.contains(e.target as Node)) hide(false);
    };
    const onScroll = (e: Event) => {
      if (menu && e.target instanceof Node && menu.contains(e.target)) return;
      // 오른쪽 클릭으로 연 메뉴는 그 자리에 둔다.
      if (!point) place();
    };
    const onResize = () => place();
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onResize);
    };
  });
</script>

<span class="menu-root" bind:this={root}>
  <button
    bind:this={trigger}
    type="button"
    class="icon-btn"
    class:xs={size === "xs"}
    class:sm={size === "sm"}
    aria-label={label}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls={open ? `${uid}-menu` : undefined}
    onclick={toggle}
    onkeydown={onTriggerKeydown}
  >
    <Icon name="dots" size={size === "xs" ? 14 : 16} />
  </button>
  {#if open}
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <ul
      id="{uid}-menu"
      bind:this={menu}
      class="menu"
      role="menu"
      aria-label={label}
      style:left="{pos?.left ?? 0}px"
      style:top="{pos?.top ?? 0}px"
      style:visibility={pos ? undefined : "hidden"}
      onkeydown={onMenuKeydown}
    >
      {#each items as item (item.label)}
        <li role="none">
          <button type="button" role="menuitem" class:danger={item.danger} onclick={() => select(item)}>
            {#if item.icon}<Icon name={item.icon} />{/if}
            {item.label}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</span>

<style>
  .menu-root {
    position: relative;
    display: inline-flex;
    flex: none;
  }

  /* 위쪽 바 탭 안의 작은 ⋯ */
  .icon-btn.xs {
    width: 24px;
    height: 24px;
    border-radius: var(--r-badge);
    color: var(--text-3);
  }

  .icon-btn.xs:hover,
  .icon-btn.xs[aria-expanded="true"] {
    background: var(--fill-hover);
    color: var(--text);
  }

  .menu {
    position: fixed;
    z-index: 40;
    list-style: none;
    margin: 0;
    padding: 4px;
    min-width: 168px;
    border-radius: var(--r-box);
    background: var(--s2);
    box-shadow: var(--shadow-pop);
  }

  button[role="menuitem"] {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 34px;
    padding: 0 10px;
    border-radius: var(--r-badge);
    color: var(--text);
    font-size: 13px;
    font-weight: 400;
    white-space: nowrap;
  }

  button[role="menuitem"] :global(svg) {
    color: var(--text-2);
  }

  button[role="menuitem"]:hover,
  button[role="menuitem"]:focus-visible {
    background: rgba(255, 255, 255, 0.06);
    outline: none;
  }

  button[role="menuitem"]:focus-visible {
    box-shadow: inset 0 0 0 2px rgba(255, 255, 255, 0.5);
  }

  button[role="menuitem"].danger,
  button[role="menuitem"].danger :global(svg) {
    color: var(--red-text);
  }
</style>
