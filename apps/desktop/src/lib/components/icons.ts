/**
 * 아이콘 모양. B 디자인 시안(지금은 지움)의 SVG 경로를 옮겨 왔다. 24×24 격자, 선 아이콘.
 * `fill: true`인 것만 면을 채운다(로고의 재생 삼각형, ⋯).
 */
export interface IconShape {
  fill?: boolean;
  body: string;
}

export const ICONS = {
  /** 로고에 쓰는 채운 재생 삼각형 */
  play: { fill: true, body: '<path d="M7 4.5v15l12.5-7.5z"/>' },
  /** 선만 있는 재생 삼각형(미리 보기) */
  "play-outline": { body: '<path d="M8 5.5v13l10.5-6.5z"/>' },
  x: { body: '<path d="m6 6 12 12M18 6 6 18"/>' },
  plus: { body: '<path d="M12 5v14M5 12h14"/>' },
  /** 설정 */
  sliders: {
    body: '<path d="M4 7h9M17.5 7H20M4 17h2.5M11 17h9"/><circle cx="15.3" cy="7" r="2.2"/><circle cx="8.7" cy="17" r="2.2"/>',
  },
  /** 설정(톱니바퀴, 위쪽 바) */
  gear: {
    body: '<path d="M10.49 5.47L10.72 2.89L13.28 2.89L13.51 5.47A6.7 6.7 0 0 1 15.55 6.32L17.54 4.65L19.35 6.46L17.68 8.45A6.7 6.7 0 0 1 18.53 10.49L21.11 10.72L21.11 13.28L18.53 13.51A6.7 6.7 0 0 1 17.68 15.55L19.35 17.54L17.54 19.35L15.55 17.68A6.7 6.7 0 0 1 13.51 18.53L13.28 21.11L10.72 21.11L10.49 18.53A6.7 6.7 0 0 1 8.45 17.68L6.46 19.35L4.65 17.54L6.32 15.55A6.7 6.7 0 0 1 5.47 13.51L2.89 13.28L2.89 10.72L5.47 10.49A6.7 6.7 0 0 1 6.32 8.45L4.65 6.46L6.46 4.65L8.45 6.32A6.7 6.7 0 0 1 10.49 5.47z"/><circle cx="12" cy="12" r="2.8"/>',
  },
  refresh: {
    body: '<path d="M20 11a8 8 0 0 0-14.5-3.8L4 9"/><path d="M4 4.5V9h4.5"/><path d="M4 13a8 8 0 0 0 14.5 3.8L20 15"/><path d="M20 19.5V15h-4.5"/>',
  },
  server: {
    body: '<rect x="3.5" y="4" width="17" height="7" rx="2"/><rect x="3.5" y="13" width="17" height="7" rx="2"/><path d="M7.2 7.5h.01M7.2 16.5h.01"/>',
  },
  /** 지문 확인됨 */
  shield: { body: '<path d="M12 3.5 19 6v5.5c0 4.2-2.8 7.3-7 9-4.2-1.7-7-4.8-7-9V6l7-2.5z"/><path d="m9 12 2.2 2.2 4-4.2"/>' },
  key: { body: '<circle cx="8" cy="15.5" r="3.5"/><path d="M10.6 13 19 4.5M15.3 8.2l2.5 2.5"/>' },
  /** ⋯ */
  dots: {
    fill: true,
    body: '<circle cx="6" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="18" cy="12" r="1.5"/>',
  },
  pencil: { body: '<path d="m4 20 1-4.2L16.3 4.5a2.2 2.2 0 0 1 3.2 3.2L8.2 19 4 20z"/><path d="m14.5 6.3 3.2 3.2"/>' },
  trash: {
    body: '<path d="M4 7h16"/><path d="M9.5 7V4.5h5V7"/><path d="m6.5 7 .9 12.5h9.2L17.5 7"/><path d="M10.2 11v5M13.8 11v5"/>',
  },
  /** 삼각 경고 */
  alert: { body: '<path d="M12 4.2 21 19.5H3L12 4.2z"/><path d="M12 10v4.3M12 16.9h.01"/>' },
  check: { body: '<path d="m5 12.5 4.5 4.5L19 7.5"/>' },
  eye: { body: '<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="2.8"/>' },
  lock: { body: '<rect x="4.5" y="11" width="15" height="9.5" rx="2.2"/><path d="M8 11V7.5a4 4 0 0 1 8 0V11"/>' },
  /** (•) 방송 */
  broadcast: {
    body: '<circle cx="12" cy="12" r="1.8"/><path d="M8.3 8.3a5.2 5.2 0 0 0 0 7.4M15.7 8.3a5.2 5.2 0 0 1 0 7.4"/><path d="M5.3 5.3a9.5 9.5 0 0 0 0 13.4M18.7 5.3a9.5 9.5 0 0 1 0 13.4"/>',
  },
  "arrow-left": { body: '<path d="M19 12H5"/><path d="m11 6-6 6 6 6"/>' },
  /** 나가기(문 밖으로 화살표, 시청 화면) */
  exit: {
    body: '<path d="M13.5 4.5H6.7a2.2 2.2 0 0 0-2.2 2.2v10.6a2.2 2.2 0 0 0 2.2 2.2h6.8"/><path d="M10 12h10"/><path d="m16.5 8.5 3.5 3.5-3.5 3.5"/>',
  },
  /** 새 창 */
  external: {
    body: '<path d="M12.5 5H6.2A2.2 2.2 0 0 0 4 7.2v10.6A2.2 2.2 0 0 0 6.2 20h10.6a2.2 2.2 0 0 0 2.2-2.2v-6.3"/><path d="M14.5 4H20v5.5"/><path d="m20 4-8.5 8.5"/>',
  },
  /** 멈춤 */
  square: { body: '<rect x="6" y="6" width="12" height="12" rx="2"/>' },
  /** 통계(막대) */
  stats: { body: '<path d="M5 20v-8M12 20V4M19 20v-11"/>' },
  fullscreen: { body: '<path d="M4 9V4.5h4.5M20 9V4.5h-4.5M4 15v4.5h4.5M20 15v4.5h-4.5"/>' },
  volume: { body: '<path d="M4 9.5v5h3.5l4.5 4v-13l-4.5 4H4z"/><path d="M15.5 9.2a4 4 0 0 1 0 5.6"/><path d="M18.2 6.5a7.8 7.8 0 0 1 0 11"/>' },
  "volume-x": { body: '<path d="M4 9.5v5h3.5l4.5 4v-13l-4.5 4H4z"/><path d="m16 9.5 5 5M21 9.5l-5 5"/>' },
  monitor: { body: '<rect x="3" y="4" width="18" height="12.5" rx="2.2"/><path d="M8.5 20h7M12 16.5V20"/>' },
  window: { body: '<rect x="3" y="4.5" width="18" height="15" rx="2.2"/><path d="M3 9.2h18"/><path d="M6.3 6.9h.01M8.8 6.9h.01"/>' },
  globe: {
    body: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17"/><path d="M12 3.5c2.4 2.3 3.6 5.1 3.6 8.5s-1.2 6.2-3.6 8.5c-2.4-2.3-3.6-5.1-3.6-8.5S9.6 5.8 12 3.5z"/>',
  },
  /** 로컬 랜 */
  lan: {
    body: '<rect x="9" y="3.5" width="6" height="5" rx="1.2"/><rect x="3.5" y="15.5" width="6" height="5" rx="1.2"/><rect x="14.5" y="15.5" width="6" height="5" rx="1.2"/><path d="M12 8.5v3.5M6.5 15.5V12h11v3.5"/>',
  },
  "chevron-right": { body: '<path d="m9.5 6 6 6-6 6"/>' },
  copy: {
    body: '<rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2.2"/><path d="M15.5 8.5V6.2A2.2 2.2 0 0 0 13.3 4H6.2A2.2 2.2 0 0 0 4 6.2v7.1a2.2 2.2 0 0 0 2.2 2.2h2.3"/>',
  },
  /** 올리기(업로드 대역폭) */
  upload: {
    body: '<path d="M12 15.5V4.5"/><path d="m7.5 9 4.5-4.5L16.5 9"/><path d="M4.5 15v2.8a2.2 2.2 0 0 0 2.2 2.2h10.6a2.2 2.2 0 0 0 2.2-2.2V15"/>',
  },
} satisfies Record<string, IconShape>;

export type IconName = keyof typeof ICONS;
