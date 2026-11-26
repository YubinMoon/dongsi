// Rust 쪽 응답 구조체(src-tauri/src/commands.rs)와 맞춘 타입.

export interface Settings {
  displayName: string;
  installId: string;
}

export interface FingerprintView {
  short: string;
  full: string;
}

/**
 * Rust 명령이 돌려주는 오류(src-tauri/src/error.rs의 CmdError).
 * `code`와 `detail`은 화면이 따로 처리해야 하는 오류에만 있다.
 */
export interface CmdError {
  message: string;
  code?: string;
  detail?: unknown;
}

/** `code: "fingerprintMismatch"` 오류의 `detail`. 저장해 둔 지문과 지금 서버가 보낸 지문. */
export interface FingerprintMismatchDetail {
  stored: FingerprintView;
  actual: FingerprintView;
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

export interface BroadcastView {
  broadcasterId: string;
  broadcasterName: string;
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
  /** 감지된 방송(`local-…`)에서 비밀번호를 기억해 서버를 목록에 저장·기억했을 때만 있다(D-057). 목록 갱신은 `servers-changed`로 한다. */
  savedServerId?: string;
}

export interface LocalBroadcastItem {
  broadcasterId: string;
  broadcasterName: string;
  width: number;
  height: number;
  fps: number;
  locked: boolean;
  /** 그 감지 서버가 저장한 서버이고 이 방송자의 비밀번호를 기억해 뒀다. */
  passwordSaved: boolean;
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
  /** Discord 창. "캡처하는 앱의 소리만"을 쓰면 Discord 소리가 들어가므로 그 모드를 막는다. */
  discord: boolean;
  /** 게임 창 따라가기 규칙(롤 클라이언트·게임 창이면 "lol", D-069). */
  follow: FollowKind | null;
  /** 캡처 대상 모니터의 주사율(Hz). 창이면 창이 있는 모니터. 모르면 null(D-074). */
  refreshRate: number | null;
}

export type FollowKind = "lol";

/** 캡처 대상 미리 보기(capture_source_thumbnails). */
export interface SourceThumbnail {
  id: string;
  /** "data:image/jpeg;base64,…". 찍지 못했으면 null. */
  image: string | null;
  /** 최소화된 창(찍지 않는다). */
  minimized: boolean;
}

/** 영상은 하드웨어 H.264로만 보낸다(D-103). 코덱은 고르지 않는다. */
export interface VideoSettings {
  /** 출력 크기 상한(상자, D-074: FHD 1920×1080, QHD 2560×1440). 0×0이나 더 큰 상자는 백엔드가 2560×1440으로 맞춘다(D-097). */
  width: number;
  height: number;
  /** 30 또는 60(D-107). 다른 값은 백엔드가 60으로 한다. 캡처 대상의 주사율을 따르지 않는다. */
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
  /** 지금 비디오 목표(kbps). 네트워크 추정값에 맞춰 `maxKbps` 아래로 낮출 수 있다(D-081). */
  targetKbps: number;
  /** 방송 설정의 비디오 비트레이트(kbps). */
  maxKbps: number;
  /** 쓸 수 있는 대역폭 추정값(kbps, 송출 BWE(TWCC)). 아직 없으면 null. */
  estimateKbps: number | null;
  /** 네트워크 때문에 목표가 설정보다 낮다. */
  limited: boolean;
}

export interface VideoStats {
  /** 쓰는 하드웨어 H.264 인코더의 ffmpeg 이름(h264_nvenc·h264_amf·h264_qsv). 열기 전에는 빈 문자열. */
  encoder: string;
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
  /** 지금 캡처 대상의 id(`SourceView.id`). 게임 창 따라가기·화면 전환(D-086)으로 바뀐다. */
  sourceId: string;
  sourceName: string;
  /** 캡처 대상이 닫혀 다음 창을 기다리는 중(게임 창 따라가기). */
  sourceWaiting: boolean;
  audio: AudioMode;
  settings: VideoSettings;
  width: number;
  height: number;
  /** 지금 쓰는 fps. `settings.fps`(30 또는 60) 그대로다(D-107). */
  fps: number;
  locked: boolean;
  viewers: number;
  startedAt: number;
  invite: string;
  inviteWithoutPassword: string | null;
  port: number | null;
  /** 내장 서버가 열려고 한 포트(`DONGSI_PORT` 또는 7884). 전용 서버로 하는 방송이면 null. */
  requestedPort: number | null;
  /** 요청한 포트를 못 써서 다른 포트로 열었다. */
  portFallback: boolean;
  upnp: UpnpView | null;
  publish: PublishStats;
  video: VideoStats;
  audioStats: AudioStats | null;
  videoError: string | null;
}

/** 방송 목록 썸네일(D-076, `broadcast_thumbnail` 응답) */
export interface BroadcastThumbnail {
  /** 서버가 붙인 그림 번호 */
  id: number;
  /** `data:image/jpeg;base64,…` */
  image: string;
}

/** `broadcast-thumbnail` 이벤트: 그 방송의 그림 번호가 바뀌었다(null이면 그림·방송이 없어짐). */
export interface ThumbnailEvent {
  key: string;
  broadcasterId: string;
  id: number | null;
}
