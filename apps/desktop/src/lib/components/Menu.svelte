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
  }: {
    /** ⋯ 단추의 aria-label. 예: "'우리집 서버' 메뉴" */
    label: string;
    items: MenuItem[];
    /** sm: 카드 안(32×32), md: 화면 머리(34×34, 면 있음) */
    size?: "sm" | "md";
  } = $props();

  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  let trigger: HTMLButtonElement | undefined = $state();
  let menu: HTMLElement | undefined = $state();
  const uid = $props.id();

  function itemButtons(): HTMLButtonElement[] {
    return menu ? Array.from(menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')) : [];
  }

  async function show() {
    open = true;
    await tick();
    itemButtons()[0]?.focus();
  }

  function hide(returnFocus = true) {
    if (!open) return;
    open = false;
    if (returnFocus) trigger?.focus();
  }

  function toggle() {
    if (open) hide(false);
    else void show();
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

  // 바깥을 누르면 닫는다.
  $effect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (root && !root.contains(e.target as Node)) hide(false);
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => window.removeEventListener("pointerdown", onPointerDown, true);
  });
</script>

<span class="menu-root" bind:this={root}>
  <button
    bind:this={trigger}
    type="button"
    class="icon-btn"
    class:sm={size === "sm"}
    class:filled={size === "md"}
    aria-label={label}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls={open ? `${uid}-menu` : undefined}
    onclick={toggle}
    onkeydown={onTriggerKeydown}
  >
    <Icon name="dots" />
  </button>
  {#if open}
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <ul id="{uid}-menu" bind:this={menu} class="menu" role="menu" aria-label={label} onkeydown={onMenuKeydown}>
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

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 20;
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
