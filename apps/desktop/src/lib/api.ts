import { invoke } from "@tauri-apps/api/core";
import type {
  AddResult,
  BroadcastStatus,
  EncoderView,
  SourceThumbnail,
  SourceView,
  StartOptions,
  VideoSettings,
  LocalServerView,
  ParsedInput,
  ServerLive,
  ServerView,
  Settings,
  WatchAnswer,
  BroadcastThumbnail,
  NetworkPath,
} from "./types";

export { errorCode, errorDetail, errorMessage } from "./errors";
export { codecLabel, qualityLabel } from "./format";

export const api = {
  /** 첫 화면을 준비했다(레이아웃). 메인 창이 숨어 있으면 보이고 시작 창을 닫는다(splash.rs). */
  appReady: () => invoke<void>("app_ready"),
  getSettings: () => invoke<Settings>("get_settings"),
  setDisplayName: (name: string) => invoke<Settings>("set_display_name", { name }),

  listServers: () => invoke<ServerView[]>("list_servers"),
  parseServerInput: (text: string) => invoke<ParsedInput>("parse_server_input", { text }),
  addServer: (text: string, name: string | null) => invoke<AddResult>("add_server", { text, name }),
  renameServer: (id: string, name: string) => invoke<ServerView>("rename_server", { id, name }),
  removeServer: (id: string) => invoke<void>("remove_server", { id }),

  connectServer: (key: string) => invoke<ServerLive>("connect_server", { key }),
  /** 그 서버 연결을 닫는다. 열려 있는 시청도 끝난다. */
  disconnectServer: (key: string) => invoke<void>("disconnect_server", { key }),
  /** 지금 연결돼 있는(닫히지 않은) 서버 key 목록 */
  connectedServers: () => invoke<string[]>("connected_servers"),
  watchBroadcast: (
    key: string,
    broadcasterId: string,
    password: string | null,
    remember: boolean,
    offer: string,
  ) => invoke<WatchAnswer>("watch_broadcast", { key, broadcasterId, password, remember, offer }),
  stopWatch: (key: string, session: number) => invoke<void>("stop_watch", { key, session }),
  /** 그 서버 연결로 가는 지금 경로(Wi-Fi 신호 갱신용, 10초마다). 연결이 없으면 null. */
  networkPath: (key: string) => invoke<NetworkPath | null>("network_path", { key }),
  requestKeyframe: (key: string, session: number) =>
    invoke<void>("request_keyframe", { key, session }),
  scanLocal: () => invoke<LocalServerView[]>("scan_local"),
  /** 방송 타일의 썸네일(D-076). 그림이 없거나 그 서버에 연결이 없으면 null. */
  broadcastThumbnail: (key: string, broadcasterId: string) =>
    invoke<BroadcastThumbnail | null>("broadcast_thumbnail", { key, broadcasterId }),
  openWatchWindow: (key: string, broadcasterId: string, title: string) =>
    invoke<void>("open_watch_window", { key, broadcasterId, title }),
  rememberPassword: (serverId: string, broadcasterId: string, password: string) =>
    invoke<void>("remember_password", { serverId, broadcasterId, password }),

  devSampleInvite: () => invoke<string>("dev_sample_invite"),
  devLog: (message: string) => invoke<void>("dev_log", { message }),
  /** 화면이 측정한 값을 앱 로그(배포 빌드는 로그 파일)에 한 줄 남긴다. 2000바이트까지. */
  watchLog: (message: string) => invoke<void>("watch_log", { message }),

  listCaptureSources: () => invoke<SourceView[]>("list_capture_sources"),
  sourceThumbnails: (ids: string[]) => invoke<SourceThumbnail[]>("capture_source_thumbnails", { ids }),
  availableEncoders: () => invoke<EncoderView[]>("available_encoders"),
  startBroadcast: (options: StartOptions) => invoke<BroadcastStatus>("start_broadcast", { options }),
  broadcastStatus: () => invoke<BroadcastStatus | null>("broadcast_status"),
  /** 송출 화면 미리 보기(바이너리, `livePreview.decodePreview`로 읽는다). `after`는 마지막으로 받은 번호. */
  broadcastPreview: (after: number, maxWidth: number, maxHeight: number) =>
    invoke<unknown>("broadcast_preview", { after, maxWidth, maxHeight }),
  updateBroadcast: (settings: VideoSettings) =>
    invoke<BroadcastStatus>("update_broadcast", { settings }),
  switchBroadcastSource: (source: string) =>
    invoke<BroadcastStatus>("switch_broadcast_source", { source }),
  stopBroadcast: () => invoke<void>("stop_broadcast"),
};
