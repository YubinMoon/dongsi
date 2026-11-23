// 방송 화면(시작·방송 중)의 순수 로직: 계산·문구 고르기·안내 목록. 화면과 Tauri에 기대지 않는다.
import { fmtMbps } from "./bitrate";
import { codecLabel, fmt } from "./format";
import type { IconName } from "./components/icons";
import type { QualityTarget } from "./bitrateZones";
import type {
  AudioMode,
  AudioStats,
  BroadcastStatus,
  EncoderView,
  HostingKind,
  Layer,
  LayerRates,
  LayerStats,
  PublishStats,
  SourceThumbnail,
  SourceView,
  VideoCodec,
  VideoSettings,
  VideoStats,
} from "./types";

/** 방송 코덱(D-075: H.264·AV1만). 칩 순서와 같다. */
export const CODECS: VideoCodec[] = ["h264", "av1"];

const MIME: Record<VideoCodec, string> = {
  h264: "video/h264",
  av1: "video/av1",
};

export const HOSTING_LABEL: Record<HostingKind, string> = {
  lan: "로컬 랜",
  direct: "PC 직접 호스팅",
  server: "추가한 서버",
};

export const HOSTING_ICON: Record<HostingKind, IconName> = {
  lan: "lan",
  direct: "globe",
  server: "server",
};

const HOSTING_HINT: Record<HostingKind, string> = {
  lan: "이 PC가 서버가 됩니다. 같은 LAN·Tailscale의 친구가 볼 수 있습니다.",
  direct: "UPnP로 공유기 포트를 열어 인터넷에서도 볼 수 있게 합니다.",
  server: "전용 서버에서 방송합니다. 여러 명이 동시에 방송할 수 있습니다.",
};

/** 호스팅 방식 아래 설명. "추가한 서버"인데 서버가 하나도 없으면 그 사실을 알린다. */
export function hostingHint(hosting: HostingKind, serverCount: number): string {
  if (hosting === "server" && serverCount === 0) return "추가한 서버가 없습니다.";
  return HOSTING_HINT[hosting];
}

export const AUDIO_LABELS: Record<AudioMode, string> = {
  systemExceptDiscord: "시스템 소리 (Discord 제외)",
  app: "캡처하는 앱의 소리만",
  system: "시스템 소리 전체 (Discord 포함)",
  none: "소리 없음",
  testTone: "테스트 톤 (A/V 동기 측정)",
};

/** 소리 select에 보일 모드. 테스트 톤은 개발 빌드에서만. */
export function audioModes(dev: boolean): AudioMode[] {
  const all = Object.keys(AUDIO_LABELS) as AudioMode[];
  return dev ? all : all.filter((m) => m !== "testTone");
}

export type NoteTone = "ok" | "warn" | "info";

export interface AudioNote {
  tone: NoteTone;
  icon: IconName;
  text: string;
}

/** 소리 모드별 안내 한 줄. Discord 소리를 빼는 모드만 초록, 섞이는 모드는 호박색. */
export function audioNote(mode: AudioMode): AudioNote {
  switch (mode) {
    case "systemExceptDiscord":
      return { tone: "ok", icon: "check", text: "PC에서 나는 소리를 보내되, Discord 소리는 방송에 넣지 않습니다." };
    case "system":
      return { tone: "warn", icon: "alert", text: "Discord 소리도 방송에 들어갑니다." };
    case "app":
      return { tone: "info", icon: "volume", text: "캡처하는 앱의 소리만 보냅니다." };
    case "none":
      return { tone: "info", icon: "volume-x", text: "소리 없이 화면만 보냅니다." };
    case "testTone":
      return { tone: "info", icon: "volume", text: "개발용: 화면 소리 대신 테스트 톤을 보냅니다." };
  }
}

// ---------- 캡처 대상 ----------

export interface SourceGroup {
  kind: SourceView["kind"];
  label: string;
  items: SourceView[];
}

const GROUP_LABEL: Record<SourceView["kind"], string> = {
  monitor: "화면",
  window: "창",
  test: "테스트",
};

/** 화면 → 창 → 테스트 순으로 묶는다. 빈 묶음은 뺀다. */
export function groupSources(sources: SourceView[]): SourceGroup[] {
  return (["monitor", "window", "test"] as const)
    .map((kind) => ({ kind, label: GROUP_LABEL[kind], items: sources.filter((s) => s.kind === kind) }))
    .filter((g) => g.items.length > 0);
}

/** 소스 타일 아래 보조 줄: "주 화면 · 3840×2160", "화면 · 1920×1080", "eldenring.exe · 2560×1440". */
export function sourceSub(s: SourceView): string {
  const size = `${s.width}×${s.height}`;
  switch (s.kind) {
    case "monitor":
      return `${s.primary ? "주 화면" : "화면"} · ${size}`;
    case "test":
      return `개발용 · ${size}`;
    case "window":
      return `${s.processName ?? "창"} · ${size}`;
  }
}

/** 캡처 대상 미리 보기를 다시 찍는 간격(ms). 대상 10개 남짓을 찍는 데 0.3초쯤 걸린다. */
export const THUMBNAIL_REFRESH_MS = 3000;

/** 미리 보기를 찍을 대상 id. 테스트 패턴은 찍지 않는다. */
export function thumbnailIds(sources: SourceView[]): string[] {
  return sources.filter((s) => s.kind !== "test").map((s) => s.id);
}

/**
 * 새로 찍은 미리 보기를 합친다. ids에 없는 대상은 버린다.
 * 이번에 그림을 못 받았으면(최소화 아님) 지난 그림을 그대로 둔다(한 번 늦은 프레임에 깜빡이지 않게).
 */
export function mergeThumbnails(
  prev: Record<string, SourceThumbnail>,
  next: SourceThumbnail[],
  ids: string[],
): Record<string, SourceThumbnail> {
  const out: Record<string, SourceThumbnail> = {};
  for (const id of ids) {
    const t = next.find((x) => x.id === id);
    if (t && (t.image || t.minimized)) out[id] = t;
    else if (prev[id]?.image) out[id] = prev[id];
  }
  return out;
}

/** 이 대상에 "캡처하는 앱의 소리만"을 쓸 수 있나: 창이고, Discord 창이 아니다(Discord 소리는 넣지 않는다). */
export function appAudioAllowed(s: SourceView | null | undefined): boolean {
  return s?.kind === "window" && !s.discord;
}

/**
 * 캡처 대상을 바꿀 때 소리 모드. 앱 소리를 쓸 수 있는 창이면 그 앱의 소리만.
 * 그 밖의 대상인데 지금이 "앱 소리"면 창을 고르기 전 모드(before)로 되돌린다. 나머지는 그대로.
 */
export function audioForSource(s: SourceView | null | undefined, current: AudioMode, before: AudioMode): AudioMode {
  if (appAudioAllowed(s)) return "app";
  if (current === "app") return before === "app" ? "systemExceptDiscord" : before;
  return current;
}

/** 게임 창 따라가기 안내(롤 창을 골랐을 때만). 소리는 "앱 소리"일 때만 따라간다. */
export function followNote(s: SourceView | null | undefined, audio: AudioMode): string | null {
  if (s?.follow !== "lol") return null;
  const what = audio === "app" ? "게임 화면과 소리로" : "게임 화면으로";
  return `롤 창: 게임이 시작되면 ${what} 넘어가고, 게임이 끝나면 클라이언트로 돌아옵니다.`;
}

/** 방송 중 화면 전환(D-086)에서 고를 수 있는 대상: 테스트 패턴은 뺀다(방송 도중 바꿀 수 없다). */
export function switchableSources(sources: SourceView[]): SourceView[] {
  return sources.filter((s) => s.kind !== "test");
}

/** 방송 중 화면을 바꿀 수 있나: 테스트 패턴 방송은 캡처 대상을 바꿀 수 없다. */
export function canSwitchSource(status: Pick<BroadcastStatus, "sourceId">): boolean {
  return status.sourceId !== "test";
}

/**
 * 화면 전환 대화 상자의 안내 줄(D-086). 소리는 백엔드가 시작 화면과 같은 규칙으로 바꾼다:
 * 앱 소리를 쓸 수 있는 창이면 그 앱의 소리만, 그 밖의 대상인데 지금 "앱 소리"면 창을 고르기 전 방식으로.
 * current가 null이면 소리가 없는 방송(오디오가 시작되지 않음)이라 소리는 그대로다. 롤 창이면 따라가기 안내를 더한다.
 */
export function switchNotes(s: SourceView | null | undefined, current: AudioMode | null): string[] {
  if (!s) return [];
  const keeps = current === null || current === "none" || current === "testTone";
  const app = !keeps && appAudioAllowed(s);
  const notes: string[] = [];
  if (app) notes.push("소리는 이 앱의 소리만 보냅니다.");
  else if (current === "app") notes.push("소리는 창을 고르기 전 방식으로 돌아갑니다.");
  const follow = followNote(s, app ? "app" : (current ?? "none"));
  if (follow) notes.push(follow);
  return notes;
}

/** 목록을 새로 읽은 뒤 고를 소스: 지금 것이 아직 있으면 그대로, 아니면 주 화면, 아니면 첫 번째. */
export function pickSourceId(sources: SourceView[], current: string): string {
  if (sources.some((s) => s.id === current)) return current;
  return (sources.find((s) => s.kind === "monitor" && s.primary) ?? sources[0])?.id ?? "";
}

// ---------- 화질·코덱 ----------

export interface CodecState {
  ok: boolean;
  /** 못 쓰는 이유("인코더 없음", "재생 불가"). 쓸 수 있으면 null. */
  reason: string | null;
  /** 쓸 수 있을 때 인코더 설명("h264_nvenc · 하드웨어"). 아직 모르면 빈 문자열. */
  encoder: string;
}

/**
 * 코덱을 쓸 수 있는가. encoders가 null이면 아직 확인 중이라 인코더 쪽은 막지 않는다.
 * receivable이 비어 있으면(getCapabilities를 못 읽음) 재생 쪽은 막지 않는다.
 */
export function codecUsable(
  codec: VideoCodec,
  encoders: EncoderView[] | null,
  receivable: ReadonlySet<string>,
): CodecState {
  const enc = encoders?.find((e) => e.codec === codec);
  if (encoders && !enc) return { ok: false, reason: "인코더 없음", encoder: "" };
  if (receivable.size > 0 && !receivable.has(MIME[codec])) {
    return { ok: false, reason: "재생 불가", encoder: "" };
  }
  return { ok: true, reason: null, encoder: enc ? `${enc.name}${enc.hardware ? " · 하드웨어" : ""}` : "" };
}

// ---------- simulcast(D-098) ----------

export const LAYER_LABELS: Record<Layer, string> = { high: "상", mid: "중", low: "하" };
const LAYER_ORDER: Layer[] = ["high", "mid", "low"];
/** simulcast로 올리는 양 / 설정 비트레이트(상 1 + 중 1/2 + 하 1/4). */
export const SIMULCAST_UPLOAD_FACTOR = 1.75;

export interface SimulcastState {
  /** 켤 수 있다. 인코더를 아직 모르면 막지 않는다(백엔드가 다시 본다). */
  available: boolean;
  /** 못 켜는 이유. 켤 수 있으면 null. */
  reason: string | null;
}

/** simulcast를 켤 수 있는가: 고른 코덱의 인코더가 하드웨어여야 한다(사용자 결정, D-098). */
export function simulcastAvailability(codec: VideoCodec, encoders: EncoderView[] | null): SimulcastState {
  const enc = encoders?.find((e) => e.codec === codec);
  if (!enc || enc.hardware) return { available: true, reason: null };
  return { available: false, reason: `소프트웨어 인코더(${enc.name})라 켤 수 없습니다.` };
}

/** 방송 설정의 simulcast 체크 상자 아래 한 줄. */
export function simulcastNote(on: boolean, state: SimulcastState, hosting: HostingKind, kbps: number): AudioNote {
  const local = hosting === "lan" || hosting === "direct";
  if (!state.available) {
    return { tone: "warn", icon: "alert", text: `${state.reason} 모든 시청자가 같은 화질을 받습니다.` };
  }
  if (!on) {
    return {
      tone: "info",
      icon: "sliders",
      text: local
        ? "모든 시청자가 같은 화질을 받고, 가장 느린 시청자의 회선에 맞춰 화질을 낮춥니다."
        : "모든 시청자가 같은 화질을 받습니다. 회선이 느린 시청자는 끊길 수 있습니다.",
    };
  }
  const up = fmtMbps(kbps * SIMULCAST_UPLOAD_FACTOR);
  return {
    tone: "info",
    icon: "sliders",
    text: local
      ? "같은 해상도로 비트레이트 1/2·1/4 화질도 같이 만들어 시청자마다 회선에 맞는 화질을 보냅니다. 인코딩을 셋 같이 해서 GPU를 조금 더 씁니다."
      : `같은 해상도로 비트레이트 1/2·1/4 화질도 같이 보내 시청자마다 회선에 맞는 화질을 받습니다. 서버로 올리는 양은 약 ${up}입니다.`,
  };
}

/** 계층별 목표: "상 7.00 Mbps · 중 3.50 Mbps · 하 뺌". */
export function layerRatesText(rates: LayerRates): string {
  return LAYER_ORDER.map((l, i) => `${LAYER_LABELS[l]} ${rates[i] > 0 ? fmtMbps(rates[i]) : "뺌"}`).join(" · ");
}

/** 고른 코덱을 못 쓰면 쓸 수 있는 첫 코덱으로. 하나도 없으면 그대로. */
export function firstUsableCodec(
  current: VideoCodec,
  encoders: EncoderView[] | null,
  receivable: ReadonlySet<string>,
): VideoCodec {
  if (codecUsable(current, encoders, receivable).ok) return current;
  return CODECS.find((c) => codecUsable(c, encoders, receivable).ok) ?? current;
}

/** 시청 앱(WebView2)이 받을 수 있는 mimeType 모음. 읽지 못하면 빈 집합. */
export function receivableFrom(caps: { codecs: { mimeType: string }[] } | null | undefined): Set<string> {
  return new Set((caps?.codecs ?? []).map((c) => c.mimeType.toLowerCase()));
}

/** 코덱 칩 목록(못 쓰는 것은 줄 긋고 이유를 단다). */
export function codecOptions(
  encoders: EncoderView[] | null,
  receivable: ReadonlySet<string>,
): { value: string; label: string; disabled: boolean; reason?: string }[] {
  return CODECS.map((c) => {
    const u = codecUsable(c, encoders, receivable);
    return { value: c, label: codecLabel(c), disabled: !u.ok, reason: u.reason ?? undefined };
  });
}

/** 코덱 칩 아래 인코더 줄: "h264_nvenc · 하드웨어 · 출력 1920×1080 (원본 2560×1440)". */
export function encoderLine(
  codec: VideoCodec,
  encoders: EncoderView[] | null,
  receivable: ReadonlySet<string>,
  target: QualityTarget | null,
  source: Pick<SourceView, "width" | "height"> | null,
): string {
  if (encoders === null) return "인코더를 확인하는 중…";
  const u = codecUsable(codec, encoders, receivable);
  const parts = [u.ok ? u.encoder : (u.reason ?? "")];
  if (source && target) {
    const same = target.width === source.width && target.height === source.height;
    parts.push(`출력 ${target.width}×${target.height}${same ? "" : ` (원본 ${source.width}×${source.height})`}`);
  }
  return parts.filter(Boolean).join(" · ");
}

// ---------- 해상도·주사율(D-074) ----------

/** 화면이 고르는 화질(비트레이트 뺀 VideoSettings). 크기는 상자 상한(FHD·QHD), fps 0 = 원본. */
export type QualityChoice = Omit<VideoSettings, "bitrateKbps">;

export interface ResolutionOption {
  value: string;
  label: string;
  width: number;
  height: number;
}

/** 출력 크기 상한(D-097, Rust `VideoSettings::MAX_WIDTH`·`MAX_HEIGHT`). 4K는 없다. */
export const MAX_WIDTH = 2560;
export const MAX_HEIGHT = 1440;

/** 해상도 칩(D-097: 원본 칩 없음. 원본에 상한을 걸면 QHD와 같아진다). */
export const RESOLUTIONS: ResolutionOption[] = [
  { value: "1920x1080", label: "FHD", width: 1920, height: 1080 },
  { value: "2560x1440", label: "QHD", width: MAX_WIDTH, height: MAX_HEIGHT },
];

/** 주사율 칩. 원본 = 0(백엔드가 캡처 대상 모니터의 주사율로 풀어냄, 최대 240). */
export const FRAME_RATES: { value: string; label: string; fps: number }[] = [
  { value: "30", label: "30", fps: 30 },
  { value: "60", label: "60", fps: 60 },
  { value: "144", label: "144", fps: 144 },
  { value: "0", label: "원본", fps: 0 },
];

/** 처음 고르는 화질: FHD · 60fps · H.264. */
export const DEFAULT_QUALITY: QualityChoice = { codec: "h264", width: 1920, height: 1080, fps: 60 };

/** 주사율을 모를 때(모니터 정보 없음, 테스트 패턴) 쓰는 값. */
export const FALLBACK_FPS = 60;
const MAX_FPS = 240;

/** 해상도 칩 값("1920x1080"). 표에 없는 크기면 칩이 하나도 켜지지 않는다. */
export function resolutionValue(width: number, height: number): string {
  return `${width}x${height}`;
}

function even(v: number): number {
  return Math.max(2, v - (v % 2));
}

/** 상자 한 축: 0(제한 없음)이나 상한보다 큰 값은 상한으로. */
function capSide(v: number, max: number): number {
  return v === 0 ? max : Math.min(v, max);
}

/**
 * 원본 크기에서 실제 출력 크기. Rust `VideoSettings::output_size`와 같은 규칙:
 * 상자(2560×1440을 넘지 않음, 0이면 그 축은 상한) 안에 비율을 유지해 맞추고, 원본이 더 작으면 원본 그대로, 짝수로.
 */
export function outputSize(boxW: number, boxH: number, srcW: number, srcH: number): { width: number; height: number } {
  const sw = Math.max(2, srcW);
  const sh = Math.max(2, srcH);
  const maxW = capSide(boxW, MAX_WIDTH);
  const maxH = capSide(boxH, MAX_HEIGHT);
  if (sw <= maxW && sh <= maxH) return { width: even(sw), height: even(sh) };
  const scale = Math.min(maxW / sw, maxH / sh);
  return { width: even(Math.round(sw * scale)), height: even(Math.round(sh * scale)) };
}

/**
 * 실제로 나갈 fps. 0(원본)이면 모니터 주사율(모르면 60). 모니터보다 높게 골라도 캡처는 모니터 주사율까지만 나온다.
 */
export function resolvedFps(fps: number, refreshRate: number | null | undefined): number {
  const monitor = typeof refreshRate === "number" && refreshRate > 0 ? Math.round(refreshRate) : null;
  const want = fps > 0 ? fps : (monitor ?? FALLBACK_FPS);
  return Math.min(MAX_FPS, Math.max(1, monitor ? Math.min(want, monitor) : want));
}

/** 화질 계산에 쓰는 캡처 대상 정보. */
export type SourceShape = Pick<SourceView, "width" | "height" | "refreshRate">;

/**
 * 고른 화질 + 캡처 대상 → 실제로 나갈 코덱·크기·fps(비트레이트 구간 계산용).
 * 대상을 아직 모르면 상자 크기(0이면 FHD)와 고른 fps(원본이면 60)로 본다.
 */
export function qualityTarget(q: QualityChoice, source: SourceShape | null): QualityTarget {
  const size = source
    ? outputSize(q.width, q.height, source.width, source.height)
    : { width: q.width || DEFAULT_QUALITY.width, height: q.height || DEFAULT_QUALITY.height };
  return { codec: q.codec, ...size, fps: resolvedFps(q.fps, source?.refreshRate) };
}

/**
 * 방송 중 캡처 대상 정보. 크기는 원본 크기(통계의 sourceWidth, 첫 프레임 전이면 출력 크기).
 * 모니터 주사율은 "원본"으로 방송 중일 때의 실제 fps로 안다. 고른 fps보다 실제 fps가 낮아도 그것이 모니터 주사율이다.
 */
export function liveSource(
  s: Pick<BroadcastStatus, "width" | "height" | "fps" | "settings"> & { video: Pick<VideoStats, "sourceWidth" | "sourceHeight"> },
): SourceShape {
  const fps = typeof s.fps === "number" && s.fps > 0 ? s.fps : null;
  const refreshRate = fps && (s.settings.fps === 0 || fps < s.settings.fps) ? fps : null;
  return {
    width: s.video.sourceWidth > 0 ? s.video.sourceWidth : s.width,
    height: s.video.sourceHeight > 0 ? s.video.sourceHeight : s.height,
    refreshRate,
  };
}

/** 주사율 칩 아래 안내. 모니터보다 높게 골랐거나 "원본"일 때만. */
export function fpsHint(fps: number, refreshRate: number | null | undefined): string | null {
  const monitor = typeof refreshRate === "number" && refreshRate > 0 ? Math.round(refreshRate) : null;
  if (fps === 0) {
    return monitor ? `원본: 이 화면의 주사율(${monitor}Hz)로 보냅니다.` : "원본: 캡처하는 화면의 주사율로 보냅니다.";
  }
  if (monitor && fps > monitor) return `이 화면은 ${monitor}Hz라 ${fps}fps를 골라도 ${monitor}fps까지만 나옵니다.`;
  return null;
}

export function settingsFor(q: QualityChoice, kbps: number): VideoSettings {
  return { codec: q.codec, width: q.width, height: q.height, fps: q.fps, bitrateKbps: kbps };
}

/** 아래 막대 요약: "1920×1080 60fps · H.264 · 로컬 랜". 서버에서 방송하면 서버 이름을 쓴다. */
export function setupSummary(
  target: QualityTarget | null,
  codec: VideoCodec,
  hosting: HostingKind,
  serverName: string | null,
): string {
  const place = hosting === "server" ? (serverName ?? HOSTING_LABEL.server) : HOSTING_LABEL[hosting];
  const size = target ? `${target.width}×${target.height} ${target.fps}fps` : null;
  return [size, codecLabel(codec), place].filter(Boolean).join(" · ");
}

/**
 * 마지막 값만 보내는 직렬 보내기(방송 중 자동 적용). 보내는 중에 새 값이 오면 기다렸다가 마지막 것 하나만 보낸다.
 * 바로 전에 넣은 값과 같으면(Enter 뒤 포커스 빠짐 등) 다시 보내지 않는다. 보내기가 실패하면(send가 false)
 * 기다리던 값을 버리고 같은 값도 다시 보낼 수 있게 한다. 다 보내면 onIdle(마지막 보내기가 성공했나)을 부른다.
 */
export class LatestOnly<T> {
  private busy = false;
  private next: { value: T } | null = null;
  private last: string | null = null;

  constructor(
    private readonly send: (value: T) => Promise<boolean>,
    private readonly onIdle?: (ok: boolean) => void,
  ) {}

  get sending(): boolean {
    return this.busy;
  }

  /** 이미 적용된 값을 알려 둔다(같은 값을 다시 보내지 않게). */
  mark(value: T): void {
    if (!this.busy) this.last = JSON.stringify(value);
  }

  /** 값을 넣는다. 돌려주는 약속은 이번에 시작한 보내기가 모두 끝나면 풀린다(이미 보내는 중이면 바로). */
  async push(value: T): Promise<void> {
    const key = JSON.stringify(value);
    if (key === this.last) return;
    this.last = key;
    if (this.busy) {
      this.next = { value };
      return;
    }
    this.busy = true;
    let v: T | null = value;
    let ok = true;
    try {
      while (v !== null) {
        ok = await this.send(v).catch(() => false);
        if (!ok) {
          this.next = null;
          this.last = null;
        }
        v = this.next ? this.next.value : null;
        this.next = null;
      }
    } finally {
      this.busy = false;
    }
    this.onIdle?.(ok);
  }
}

/** 시작 전에 막을 오류. 없으면 null. */
export function startError(sourceId: string, hosting: HostingKind, serverId: string): string | null {
  if (!sourceId) return "캡처할 화면이나 창을 골라 주세요";
  if (hosting === "server" && !serverId) return "방송할 서버를 골라 주세요";
  return null;
}

// ---------- 방송 중 ----------

/** "공유" 줄: "PC 직접 호스팅 · 지우 · 포트 7884". */
export function shareLine(s: Pick<BroadcastStatus, "hosting" | "serverName" | "port">): string {
  const parts = [HOSTING_LABEL[s.hosting], s.serverName];
  if (s.port) parts.push(`포트 ${s.port}`);
  return parts.filter(Boolean).join(" · ");
}

export interface ShareNotice {
  id: "port" | "upnp" | "doubleNat" | "upnpError";
  tone: "ok" | "warn";
  text: string;
  /** 본문 뒤에 고정폭으로 붙는 값(UPnP 외부 주소) */
  code?: string;
}

/** 공유 칸 위에 쌓을 안내(포트 대체, UPnP 상태). 시안 순서: 포트 → UPnP → 이중 NAT. */
export function shareNotices(
  s: Pick<BroadcastStatus, "port" | "requestedPort" | "portFallback" | "upnp">,
): ShareNotice[] {
  const list: ShareNotice[] = [];
  // 일부러 다른 포트를 고른 경우(DONGSI_PORT)는 대체가 아니다. 백엔드가 알려 준 portFallback만 믿는다.
  if (s.portFallback && s.port && s.requestedPort) {
    list.push({
      id: "port",
      tone: "warn",
      text: `${s.requestedPort}번 포트를 다른 프로그램(같은 PC의 전용 서버 등)이 쓰고 있어 ${s.port}번으로 열었습니다. 이 방송은 같은 LAN·Tailscale의 자동 감지 목록에 나타나지 않으니 초대 코드로 알려 주세요.`,
    });
  }
  const upnp = s.upnp;
  if (upnp) {
    if (upnp.external) {
      list.push({ id: "upnp", tone: "ok", text: "UPnP 포트 열림: ", code: upnp.external });
      if (upnp.doubleNat) {
        list.push({
          id: "doubleNat",
          tone: "warn",
          text: "외부 IP가 사설 대역입니다(이중 NAT 또는 CGNAT). 인터넷에서는 접속하지 못할 수 있습니다.",
        });
      }
    } else if (upnp.error) {
      list.push({ id: "upnpError", tone: "warn", text: upnp.error });
    }
  }
  return list;
}

/** 음량 막대 비율(0~1). -60 dBFS 이하는 0, 0 dBFS는 1. 값이 없으면 0. */
export function levelRatio(dbfs: number | null | undefined): number {
  if (typeof dbfs !== "number" || !Number.isFinite(dbfs)) return 0;
  return Math.min(1, Math.max(0, (dbfs + 60) / 60));
}

/** 음량 글자: "-18 dBFS". 값이 없으면 "–". */
export function levelText(dbfs: number | null | undefined): string {
  const t = fmt(dbfs);
  return t === "–" ? t : `${t} dBFS`;
}

export interface StatRow {
  label: string;
  value: string;
  tone?: "ok" | "warn" | "muted";
}

/** 영상 열. 버림은 인코더가 버린 것과 송출이 버린 것의 합. */
export function videoRows(s: Pick<BroadcastStatus, "sourceName" | "width" | "height" | "settings"> & {
  sourceWaiting?: boolean;
  video: VideoStats;
  publish: PublishStats;
  /** simulcast 하위 계층(중·하). 없거나 비었으면 줄을 넣지 않는다. */
  layers?: LayerStats[];
}): StatRow[] {
  const v = s.video;
  const rows: StatRow[] = [
    s.sourceWaiting
      ? { label: "캡처", value: `${s.sourceName} · 다음 창을 기다리는 중`, tone: "warn" }
      : { label: "캡처", value: s.sourceName },
    { label: "출력", value: `${s.width}×${s.height} · ${codecLabel(s.settings.codec)}` },
    { label: "인코더", value: `${v.encoder || "–"} (${v.hardware ? "하드웨어" : "소프트웨어"})` },
    {
      label: "프레임",
      value: `캡처 ${fmt(v.captureFps)} / 인코딩 ${fmt(v.encodeFps)} fps · 버림 ${v.droppedFrames + s.publish.droppedFrames}`,
    },
    {
      label: "인코딩",
      value: `평균 ${fmt(v.avgEncodeMs, 1)} ms · 변환 ${fmt(v.avgConvertMs, 1)} ms (${v.gpuConvert ? "GPU" : "CPU"})`,
    },
    { label: "캡처→출력", value: `${fmt(v.avgLatencyMs, 1)} ms` },
  ];
  const layers = s.layers ?? [];
  if (layers.length > 0) rows.push(layerEncodeRow(layers));
  return rows;
}

/** simulcast 하위 계층 인코딩 줄: "중 60 fps · 하 60 fps"(쉬는 계층은 "쉼"). 오류나 소프트웨어 쉼은 경고. */
function layerEncodeRow(layers: LayerStats[]): StatRow {
  const label = "계층 인코딩";
  const failed = layers.find((l) => l.lastError);
  if (failed) return { label, value: `${LAYER_LABELS[failed.layer]}: ${failed.lastError}`, tone: "warn" };
  if (layers.some((l) => l.softwarePaused)) return { label, value: "소프트웨어 인코더라 쉼", tone: "warn" };
  const value = layers
    .map((l) => `${LAYER_LABELS[l.layer]} ${l.targetKbps > 0 ? `${fmt(l.encodeFps)} fps` : "쉼"}`)
    .join(" · ");
  return { label, value };
}

/** 송출 열. 손실은 비율(0~1)을 %로. 목표가 네트워크 때문에 설정보다 낮으면 경고 색으로(D-081). */
export function sendRows(p: PublishStats): StatRow[] {
  const estimate =
    p.estimateKbps === null
      ? "–"
      : p.rembKbps !== null && p.rembKbps <= p.estimateKbps
        ? `${fmtMbps(p.estimateKbps)} · 가장 느린 시청자 기준`
        : fmtMbps(p.estimateKbps);
  // simulcast면 목표 대신 계층별 목표(뺀 계층 포함)를 보인다(D-098).
  const target: StatRow = p.layers
    ? { label: "계층", value: layerRatesText(p.layers), ...(p.limited ? { tone: "warn" as const } : {}) }
    : p.limited
      ? { label: "목표", value: `${fmtMbps(p.targetKbps)} · 네트워크에 맞춰 낮춤`, tone: "warn" }
      : { label: "목표", value: fmtMbps(p.targetKbps) };
  return [
    { label: "비디오", value: fmtMbps(p.videoKbps) },
    target,
    { label: "추정 대역폭", value: estimate },
    // audio_kbps는 RED 사본을 포함한 송신 바이트라 Opus 설정값의 약 3배로 보인다(D-087).
    { label: "오디오", value: `${fmt(p.audioKbps)} kbps${p.audioKbps > 0 ? " · RED 포함" : ""}` },
    { label: "RTT", value: `${fmt(p.rttMs)} ms` },
    { label: "손실", value: `${fmt((p.loss ?? 0) * 100, 1)}%` },
    { label: "키프레임 요청", value: String(p.keyframeRequests) },
  ];
}

/** 화질 변경 패널의 안내: 네트워크 때문에 설정보다 낮게 보내는 중이면 알린다(D-081). 아니면 null. */
export function networkLimitHint(
  p: Pick<PublishStats, "limited" | "targetKbps" | "maxKbps" | "rembKbps"> & Partial<Pick<PublishStats, "layers">>,
): string | null {
  if (!p.limited) return null;
  if (p.layers) {
    // simulcast: 업로드가 모자라면 하 → 중 순서로 빼고, 그래도 모자라면 상을 낮춘다(D-098).
    const [high, mid, low] = p.layers;
    if (high < p.maxKbps) {
      return `업로드가 모자라 상 계층만 ${fmtMbps(high)}로 보내고 있습니다(설정 ${fmtMbps(p.maxKbps)}). 회선이 나아지면 자동으로 올립니다.`;
    }
    if (mid === 0) {
      return "업로드가 모자라 중·하 계층을 빼고 상 계층만 보내고 있습니다. 회선이 느린 시청자는 끊길 수 있습니다. 회선이 나아지면 자동으로 되돌립니다.";
    }
    if (low === 0) return "업로드가 모자라 하 계층을 빼고 보내고 있습니다. 회선이 나아지면 자동으로 되돌립니다.";
  }
  const why = p.rembKbps !== null ? "가장 느린 시청자의 회선에 맞춰" : "네트워크가 붐벼서";
  return `${why} 지금은 ${fmtMbps(p.targetKbps)}로 보내고 있습니다(설정 ${fmtMbps(p.maxKbps)}). 회선이 나아지면 자동으로 올립니다.`;
}

/** "Discord" 줄은 "Discord 제외" 모드에서만. 빼는 중이면 초록, 아니면 실행 중이 아니라는 안내. */
export function discordRow(mode: AudioMode, audio: Pick<AudioStats, "discordExcluded"> | null): StatRow | null {
  if (mode !== "systemExceptDiscord" || !audio) return null;
  return audio.discordExcluded
    ? { label: "Discord", value: "Discord 소리 빼는 중", tone: "ok" }
    : { label: "Discord", value: "Discord가 실행 중이 아님(켜지면 자동으로 뺌)", tone: "muted" };
}

/** 소리 열 첫 줄. */
export function audioModeRow(mode: AudioMode): StatRow {
  return { label: "방식", value: AUDIO_LABELS[mode] ?? mode };
}
