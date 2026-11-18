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

export interface BroadcastView {
  broadcasterId: string;
  broadcasterName: string;
  width: number;
  height: number;
  fps: number;
  locked: boolean;
  passwordSaved: boolean;
}

export interface ServerBroadcasts {
  sample: boolean;
  broadcasts: BroadcastView[];
}

export interface LocalBroadcastView {
  hostName: string;
  addr: string;
  via: "lan" | "tailscale";
  width: number;
  height: number;
  fps: number;
  locked: boolean;
}

export interface LocalBroadcasts {
  sample: boolean;
  items: LocalBroadcastView[];
}
