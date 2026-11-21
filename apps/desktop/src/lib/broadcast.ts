// 방송 화면(시작·방송 중)의 순수 로직: 계산·문구 고르기·안내 목록. 화면과 Tauri에 기대지 않는다.
import { fmtMbps } from "./bitrate";
import { codecLabel, fmt } from "./format";
import type { IconName } from "./components/icons";
import type {
  AudioMode,
  AudioStats,
  BroadcastStatus,
  EncoderView,
  HostingKind,
  PresetView,
  PublishStats,
  SourceThumbnail,
  SourceView,
  VideoCodec,
  VideoSettings,
  VideoStats,
} from "./types";

export const CODECS: VideoCodec[] = ["h264", "h265", "vp8", "vp9", "av1"];

const MIME: Record<VideoCodec, string> = {
  h264: "video/h264",
  h265: "video/h265",
  vp8: "video/vp8",
  vp9: "video/vp9",
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

/** 코덱 칩 아래 인코더 줄: "h264_nvenc · 하드웨어 · 출력 최대 1920×1080 (원본 2560×1440)". */
export function encoderLine(
  codec: VideoCodec,
  encoders: EncoderView[] | null,
  receivable: ReadonlySet<string>,
  preset: PresetView | null,
  source: SourceView | null,
): string {
  if (encoders === null) return "인코더를 확인하는 중…";
  const u = codecUsable(codec, encoders, receivable);
  const parts = [u.ok ? u.encoder : (u.reason ?? "")];
  if (source && preset && preset.width > 0) {
    parts.push(`출력 최대 ${preset.width}×${preset.height} (원본 ${source.width}×${source.height})`);
  }
  return parts.filter(Boolean).join(" · ");
}

export function presetBitrate(preset: PresetView | null, codec: VideoCodec): number | null {
  return preset ? (preset.bitrates[codec] ?? null) : null;
}

/** 입력란 값(비어 있으면 null)을 보낼 값으로. 숫자가 아니거나 0 이하면 기본값. 범위는 백엔드가 맞춘다. */
export function normalizeBitrate(value: number | null | undefined, fallback: number): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) return fallback;
  return Math.round(value);
}

export function settingsFor(preset: PresetView | null, codec: VideoCodec, kbps: number): VideoSettings {
  return {
    codec,
    width: preset?.width ?? 1920,
    height: preset?.height ?? 1080,
    fps: preset?.fps ?? 60,
    bitrateKbps: kbps,
  };
}

/** 지금 방송 설정과 같은 프리셋 id. 같은 것이 없으면 첫 프리셋. */
export function presetIdFor(presets: PresetView[], s: VideoSettings): string {
  return (
    presets.find((p) => p.width === s.width && p.height === s.height && p.fps === s.fps)?.id ??
    presets[0]?.id ??
    ""
  );
}

/** 아래 막대 요약: "1080p 60fps · H.264 · 로컬 랜". 서버에서 방송하면 서버 이름을 쓴다. */
export function setupSummary(
  preset: PresetView | null,
  codec: VideoCodec,
  hosting: HostingKind,
  serverName: string | null,
): string {
  const place = hosting === "server" ? (serverName ?? HOSTING_LABEL.server) : HOSTING_LABEL[hosting];
  return [preset?.label, codecLabel(codec), place].filter(Boolean).join(" · ");
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
  video: VideoStats;
  publish: PublishStats;
}): StatRow[] {
  const v = s.video;
  return [
    { label: "캡처", value: s.sourceName },
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
}

/** 송출 열. 손실은 비율(0~1)을 %로. */
export function sendRows(p: PublishStats): StatRow[] {
  return [
    { label: "비디오", value: fmtMbps(p.videoKbps) },
    { label: "오디오", value: `${fmt(p.audioKbps)} kbps` },
    { label: "RTT", value: `${fmt(p.rttMs)} ms` },
    { label: "손실", value: `${fmt((p.loss ?? 0) * 100, 1)}%` },
    { label: "키프레임 요청", value: String(p.keyframeRequests) },
  ];
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
