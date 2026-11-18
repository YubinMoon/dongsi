import { invoke } from "@tauri-apps/api/core";
import type {
  AddResult,
  BroadcastStatus,
  EncoderView,
  PresetView,
  SourceView,
  StartOptions,
  VideoSettings,
  LocalServerView,
  ParsedInput,
  ServerLive,
  ServerView,
  Settings,
  VideoCodec,
  WatchAnswer,
} from "./types";

/** Rust 명령이 돌려준 오류(`{ message }`)나 그 밖의 예외에서 보여 줄 문장을 꺼낸다. */
export function errorMessage(err: unknown): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

/** 화면에서 따로 처리하는 오류 코드(`passwordRequired` 등). */
export function errorCode(err: unknown): string | null {
  if (typeof err === "object" && err !== null && "code" in err) {
    return String((err as { code: unknown }).code);
  }
  return null;
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  setDisplayName: (name: string) => invoke<Settings>("set_display_name", { name }),

  listServers: () => invoke<ServerView[]>("list_servers"),
  parseServerInput: (text: string) => invoke<ParsedInput>("parse_server_input", { text }),
  addServer: (text: string, name: string | null) => invoke<AddResult>("add_server", { text, name }),
  renameServer: (id: string, name: string) => invoke<ServerView>("rename_server", { id, name }),
  removeServer: (id: string) => invoke<void>("remove_server", { id }),

  connectServer: (key: string) => invoke<ServerLive>("connect_server", { key }),
  watchBroadcast: (
    key: string,
    broadcasterId: string,
    password: string | null,
    remember: boolean,
    offer: string,
  ) => invoke<WatchAnswer>("watch_broadcast", { key, broadcasterId, password, remember, offer }),
  stopWatch: (key: string, session: number) => invoke<void>("stop_watch", { key, session }),
  requestKeyframe: (key: string, session: number) =>
    invoke<void>("request_keyframe", { key, session }),
  scanLocal: () => invoke<LocalServerView[]>("scan_local"),
  openWatchWindow: (key: string, broadcasterId: string, title: string) =>
    invoke<void>("open_watch_window", { key, broadcasterId, title }),
  rememberPassword: (serverId: string, broadcasterId: string, password: string) =>
    invoke<void>("remember_password", { serverId, broadcasterId, password }),

  devSampleInvite: () => invoke<string>("dev_sample_invite"),
  devLog: (message: string) => invoke<void>("dev_log", { message }),

  listCaptureSources: () => invoke<SourceView[]>("list_capture_sources"),
  availableEncoders: () => invoke<EncoderView[]>("available_encoders"),
  qualityPresets: () => invoke<PresetView[]>("quality_presets"),
  startBroadcast: (options: StartOptions) => invoke<BroadcastStatus>("start_broadcast", { options }),
  broadcastStatus: () => invoke<BroadcastStatus | null>("broadcast_status"),
  updateBroadcast: (settings: VideoSettings) =>
    invoke<BroadcastStatus>("update_broadcast", { settings }),
  stopBroadcast: () => invoke<void>("stop_broadcast"),
};

const CODEC_LABELS: Record<VideoCodec, string> = {
  h264: "H.264",
  h265: "H.265",
  vp8: "VP8",
  vp9: "VP9",
  av1: "AV1",
};

export function codecLabel(codec: VideoCodec): string {
  return CODEC_LABELS[codec] ?? codec;
}

/** 세로 1080 → "1080p · 60fps", 2160 → "4K · 30fps" */
export function qualityLabel(height: number, fps: number): string {
  const tier = height >= 2160 ? "4K" : `${height}p`;
  return `${tier} · ${fps}fps`;
}
