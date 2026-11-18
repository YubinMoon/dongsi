import { invoke } from "@tauri-apps/api/core";
import type {
  AddResult,
  LocalBroadcasts,
  ParsedInput,
  ServerBroadcasts,
  ServerView,
  Settings,
} from "./types";

/** Rust 명령이 돌려준 오류(`{ message }`)나 그 밖의 예외에서 보여 줄 문장을 꺼낸다. */
export function errorMessage(err: unknown): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  setDisplayName: (name: string) => invoke<Settings>("set_display_name", { name }),

  listServers: () => invoke<ServerView[]>("list_servers"),
  parseServerInput: (text: string) => invoke<ParsedInput>("parse_server_input", { text }),
  addServer: (text: string, name: string | null) => invoke<AddResult>("add_server", { text, name }),
  renameServer: (id: string, name: string) => invoke<ServerView>("rename_server", { id, name }),
  removeServer: (id: string) => invoke<void>("remove_server", { id }),

  listBroadcasts: (serverId: string) => invoke<ServerBroadcasts>("list_broadcasts", { serverId }),
  listLocalBroadcasts: () => invoke<LocalBroadcasts>("list_local_broadcasts"),
  rememberPassword: (serverId: string, broadcasterId: string, password: string) =>
    invoke<void>("remember_password", { serverId, broadcasterId, password }),

  devSampleInvite: () => invoke<string>("dev_sample_invite"),
};

/** 세로 1080 → "1080p · 60fps", 2160 → "4K · 30fps" */
export function qualityLabel(height: number, fps: number): string {
  const tier = height >= 2160 ? "4K" : `${height}p`;
  return `${tier} · ${fps}fps`;
}
