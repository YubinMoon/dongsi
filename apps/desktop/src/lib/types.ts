// Rust 쪽 응답 구조체(src-tauri/src/commands.rs)와 맞춘 타입.

export interface Settings {
  displayName: string;
  installId: string;
}

export interface FingerprintView {
  short: string;
  full: string;
}

export interface ServerView {
  id: string;
  name: string;
  addrs: string[];
  /** null이면 아직 첫 접속 전이라 지문을 확인하지 않았다. */
  fingerprint: FingerprintView | null;
  savedPasswords: number;
}

export interface ExistingServer {
  id: string;
  name: string;
}

export type ParsedInput =
  | { kind: "address"; addr: string; existing: ExistingServer | null }
  | {
      kind: "invite";
      name: string | null;
      addrs: string[];
      fingerprint: FingerprintView;
      broadcast: boolean;
      hasPassword: boolean;
      existing: ExistingServer | null;
    };

export interface AddResult {
  outcome: "added" | "updated" | "alreadyExists";
  server: ServerView;
}

export type VideoCodec = "h264" | "h265" | "vp8" | "vp9" | "av1";

export interface BroadcastView {
  broadcasterId: string;
  broadcasterName: string;
  codec: VideoCodec;
  width: number;
  height: number;
  fps: number;
  hasAudio: boolean;
  locked: boolean;
  viewers: number;
  /** 유닉스 시각(초) */
  startedAt: number;
  passwordSaved: boolean;
  /** 내 방송 */
  mine: boolean;
}

/** 접속한 서버의 상태(connect_server). */
export interface ServerLive {
  key: string;
  serverName: string;
  canPublish: boolean;
  hostOnly: boolean;
  fingerprint: FingerprintView;
  broadcasts: BroadcastView[];
}

export interface WatchAnswer {
  session: number;
  answer: string;
}

export interface LocalBroadcastItem {
  broadcasterId: string;
  broadcasterName: string;
  codec: VideoCodec;
  width: number;
  height: number;
  fps: number;
  locked: boolean;
}

/** 로컬 감지로 찾은 서버(scan_local). */
export interface LocalServerView {
  key: string;
  name: string;
  addr: string;
  via: "lan" | "tailscale" | "local";
  hostOnly: boolean;
  saved: boolean;
  broadcasts: LocalBroadcastItem[];
}

/** 서버 이벤트 페이로드 */
export interface KeyEvent {
  key: string;
}

export interface BroadcastEndedEvent {
  key: string;
  broadcasterId: string;
}

export interface ServerClosedEvent {
  key: string;
  reason: string;
}

export interface ViewerCountEvent {
  key: string;
  count: number;
}

// ---------- 방송 ----------

export type HostingKind = "server" | "lan" | "direct";
export type AudioMode = "systemExceptDiscord" | "app" | "system" | "none" | "testTone";
export type SourceKind = "monitor" | "window" | "test";

export interface SourceView {
  id: string;
  kind: SourceKind;
  name: string;
  width: number;
  height: number;
  primary: boolean;
  processName: string | null;
}

export interface EncoderView {
  codec: VideoCodec;
  name: string;
  hardware: boolean;
}

export interface PresetView {
  id: string;
  label: string;
  width: number;
  height: number;
  fps: number;
  bitrates: Record<VideoCodec, number>;
}

export interface VideoSettings {
  codec: VideoCodec;
  width: number;
  height: number;
  fps: number;
  bitrateKbps: number;
}

export interface StartOptions {
  hosting: HostingKind;
  serverId: string | null;
  source: string;
  settings: VideoSettings;
  audio: AudioMode;
  password: string | null;
}

export interface PublishStats {
  connected: boolean;
  closed: string | null;
  videoKbps: number;
  audioKbps: number;
  rttMs: number | null;
  loss: number | null;
  nacks: number;
  keyframeRequests: number;
  droppedFrames: number;
}

export interface VideoStats {
  encoder: string;
  hardware: boolean;
  codec: VideoCodec | null;
  width: number;
  height: number;
  sourceWidth: number;
  sourceHeight: number;
  captureFps: number;
  encodeFps: number;
  avgEncodeMs: number;
  bitrateKbps: number;
  droppedFrames: number;
  avgConvertMs: number;
  avgLatencyMs: number;
  gpuConvert: boolean;
  frames: number;
  keyframes: number;
}

export interface AudioStats {
  mode: string;
  discordExcluded: boolean;
  levelDbfs: number;
  packets: number;
  encoderDelaySamples: number;
  silenceFilledMs: number;
  droppedMs: number;
  lastError: string | null;
}

export interface UpnpView {
  external: string | null;
  doubleNat: boolean;
  error: string | null;
}

export interface BroadcastStatus {
  hosting: HostingKind;
  key: string;
  broadcasterId: string;
  serverName: string;
  sourceName: string;
  audio: AudioMode;
  settings: VideoSettings;
  width: number;
  height: number;
  locked: boolean;
  viewers: number;
  startedAt: number;
  invite: string;
  inviteWithoutPassword: string | null;
  port: number | null;
  upnp: UpnpView | null;
  publish: PublishStats;
  video: VideoStats;
  audioStats: AudioStats | null;
  videoError: string | null;
}
