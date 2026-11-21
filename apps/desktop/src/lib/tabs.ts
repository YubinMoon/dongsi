import { closedReasonDetail, connectFailure } from "./errors";
import type { IconName } from "./components/icons";
import type { FingerprintView, ServerView } from "./types";

/*
 * 위쪽 바의 서버 탭(D-073): 저장한 서버를 저장 순서대로 모두 탭으로 둔다.
 * 연결 안 된 서버도 탭에 남고, 상태(연결하는 중·자동 연결 실패·실패·지문 경고·끊김)를 탭에 보인다.
 * 저장하지 않은 서버(감지한 방송을 보느라 연결한 서버)는 탭에 두지 않는다(서버 화면이 없다).
 * 모든 함수는 순수 함수이고 새 값을 돌려준다.
 */

/** 연결 안 된 서버의 마지막 접속 시도 결과. 연결되면 지운다. */
export type AttemptState =
  /** 사용자가 직접(탭 메뉴·다시 연결) 시도하는 중 */
  | { kind: "connecting" }
  /** 사용자가 직접 시도했다가 실패했다(빨간 점, D-054). */
  | { kind: "failed"; message: string }
  /** 자동 연결(D-071)이 실패했다(회색 점). 뒤에서 다시 시도한다. */
  | { kind: "offline"; message: string }
  /** 인증서 지문이 저장한 것과 다르다(호박색, D-054). 사용자가 다시 연결할 때까지 자동으로 시도하지 않는다. */
  | {
      kind: "fingerprint";
      message: string;
      stored: FingerprintView | null;
      actual: FingerprintView | null;
    };

/** 탭·서버 화면이 읽는 연결 상태(`conn`의 일부). */
export interface ConnSnapshot {
  /** 연결된 서버 key(연결한 순서) */
  connected: string[];
  /** 서버 key → 마지막 접속 시도 결과 */
  card: Record<string, AttemptState | undefined>;
  /** 지금 접속을 시도하는 서버(자동 연결 포함) */
  trying: Record<string, boolean | undefined>;
  /** 사용자가 연결을 끊은 서버(이번 실행 동안 자동 연결하지 않음) */
  paused: Record<string, boolean | undefined>;
  /** 연결돼 있다가 끊긴 서버의 이유(`server-closed`) */
  closedReason: Record<string, string | undefined>;
}

/**
 * 서버 하나의 상태. 탭 표시와 서버 화면이 같이 쓴다. 위에 있는 것이 먼저다.
 * `retrying`: 뒤에서 다시 시도하는 중(표시는 그대로 두고 "다시 연결"만 막는다).
 */
export type ServerStatus =
  | { kind: "connected" }
  /** 연결하는 중(사용자가 누른 시도, 또는 아직 실패한 적 없는 자동 연결) */
  | { kind: "connecting" }
  | {
      kind: "fingerprint";
      message: string;
      /** 저장한 지문과 실제 지문을 둘 다 알 때만 있다(없으면 message만 보인다). */
      compare: { stored: FingerprintView; actual: FingerprintView } | null;
    }
  /** `message`는 짧은 문구, `title`은 툴팁에 두는 전체 문장 */
  | { kind: "failed"; message: string; title: string; retrying: boolean }
  /** 연결돼 있다가 끊겼다. `reason`은 기본 문구가 아닐 때만 있다(D-055). */
  | { kind: "closed"; reason: string | null; retrying: boolean }
  | { kind: "offline"; message: string; title: string; retrying: boolean }
  /** 사용자가 연결을 끊었다 */
  | { kind: "paused" }
  /** 아직 시도 전 */
  | { kind: "idle" };

export type ServerStatusKind = ServerStatus["kind"];

/** 서버 하나의 상태를 고른다. */
export function serverStatus(key: string, s: ConnSnapshot): ServerStatus {
  if (s.connected.includes(key)) return { kind: "connected" };
  const attempt = s.card[key];
  const retrying = s.trying[key] === true;
  if (attempt?.kind === "connecting") return { kind: "connecting" };
  if (attempt?.kind === "fingerprint") {
    const { stored, actual } = attempt;
    return { kind: "fingerprint", message: attempt.message, compare: stored && actual ? { stored, actual } : null };
  }
  if (attempt?.kind === "failed") {
    const { short, full } = connectFailure(attempt.message);
    return { kind: "failed", message: short, title: full, retrying };
  }
  // 보던 중에 끊긴 서버는 다시 연결될 때까지 "끊어졌습니다"를 보인다(뒤의 자동 재시도 실패는 조용히).
  const reason = s.closedReason[key];
  if (reason !== undefined) return { kind: "closed", reason: closedReasonDetail(reason), retrying };
  if (attempt?.kind === "offline") {
    const { short, full } = connectFailure(attempt.message);
    return { kind: "offline", message: short, title: full, retrying };
  }
  if (retrying) return { kind: "connecting" };
  if (s.paused[key]) return { kind: "paused" };
  return { kind: "idle" };
}

export interface TabView {
  id: string;
  name: string;
  active: boolean;
  status: ServerStatus;
  /** 탭에 마우스를 올리면 보이는 문장(이유 등). 없으면 null. */
  tooltip: string | null;
  /** 화면 읽기용 상태 문구(탭 이름 뒤). 연결됨이면 null. */
  statusText: string | null;
}

/** 탭 툴팁 */
export function tabTooltip(status: ServerStatus): string | null {
  switch (status.kind) {
    case "connected":
    case "idle":
      return null;
    case "connecting":
      return "연결하는 중…";
    case "fingerprint":
      return "인증서 지문이 다릅니다. 눌러서 확인하세요.";
    case "failed":
    case "offline":
      return status.title;
    case "closed":
      return status.reason ? `서버와 연결이 끊어졌습니다: ${status.reason}` : "서버와 연결이 끊어졌습니다.";
    case "paused":
      return "연결을 끊었습니다. 메뉴에서 다시 연결할 수 있습니다.";
  }
}

/** 탭 이름 뒤에 붙는 화면 읽기용 상태 */
export function tabStatusText(status: ServerStatus): string | null {
  switch (status.kind) {
    case "connected":
      return null;
    case "connecting":
      return "연결하는 중";
    case "fingerprint":
      return "인증서 지문이 다름";
    case "failed":
      return "연결 실패";
    case "closed":
      return "연결 끊어짐";
    case "offline":
      return "연결 안 됨";
    case "paused":
      return "연결 끊음";
    case "idle":
      return "연결 전";
  }
}

/** 그릴 탭: 저장한 서버를 저장 순서대로 모두. `activeId`는 지금 보는 서버 화면의 ID. */
export function computeTabs(servers: Pick<ServerView, "id" | "name">[], s: ConnSnapshot, activeId: string | null): TabView[] {
  return servers.map((server) => {
    const status = serverStatus(server.id, s);
    return {
      id: server.id,
      name: server.name,
      active: server.id === activeId,
      status,
      tooltip: tabTooltip(status),
      statusText: tabStatusText(status),
    };
  });
}

/** 탭 메뉴·서버 화면 메뉴의 항목 */
export type ServerAction = "disconnect" | "reconnect" | "rename" | "remove";

export interface ServerMenuEntry {
  action: ServerAction;
  label: string;
  icon: IconName;
  danger?: boolean;
}

/** 서버 메뉴: 연결돼 있으면 "연결 끊기", 연결하는 중이 아니면 "다시 연결", 그리고 "이름 변경"·"삭제". */
export function serverMenu(status: ServerStatus): ServerMenuEntry[] {
  const entries: ServerMenuEntry[] = [];
  if (status.kind === "connected") entries.push({ action: "disconnect", label: "연결 끊기", icon: "x" });
  else if (status.kind !== "connecting") entries.push({ action: "reconnect", label: "다시 연결", icon: "refresh" });
  entries.push({ action: "rename", label: "이름 변경", icon: "pencil" });
  entries.push({ action: "remove", label: "삭제", icon: "trash", danger: true });
  return entries;
}

// ---------- 연결 목록(연결한 순서) ----------

/** 연결에 성공한 서버를 맨 끝에 더한다(이미 있으면 그대로). */
export function addConnected(keys: string[], key: string): string[] {
  return keys.includes(key) ? [...keys] : [...keys, key];
}

/** 연결 목록에서 뺀다. */
export function removeKey(keys: string[], key: string): string[] {
  return keys.filter((k) => k !== key);
}

/**
 * 백엔드의 연결 목록(`connected_servers`)에 맞춘다. 백엔드가 기준이다.
 * 아는 키는 순서를 지키고 백엔드에 없으면 빼며, 백엔드에만 있는 키는 끝에 더한다.
 */
export function mergeBackend(keys: string[], backendKeys: string[]): string[] {
  const next = keys.filter((k) => backendKeys.includes(k));
  for (const key of backendKeys) {
    if (!next.includes(key)) next.push(key);
  }
  return next;
}
