import type { FingerprintMismatchDetail, FingerprintView } from "./types";

/** Rust 명령이 돌려준 오류(`{ message }`)나 그 밖의 예외에서 보여 줄 문장을 꺼낸다. */
export function errorMessage(err: unknown): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

/** 화면에서 따로 처리하는 오류 코드(`passwordRequired`, `fingerprintMismatch` 등). */
export function errorCode(err: unknown): string | null {
  if (typeof err === "object" && err !== null && "code" in err) {
    const code = (err as { code: unknown }).code;
    return code === undefined || code === null ? null : String(code);
  }
  return null;
}

/** 오류에 딸린 자세한 정보(`CmdError.detail`). 없으면 undefined. */
export function errorDetail(err: unknown): unknown {
  if (typeof err === "object" && err !== null && "detail" in err) {
    return (err as { detail: unknown }).detail;
  }
  return undefined;
}

function isFingerprintView(value: unknown): value is FingerprintView {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as FingerprintView).short === "string" &&
    typeof (value as FingerprintView).full === "string"
  );
}

/**
 * 지문 불일치 오류면 저장한 지문과 실제 지문을 돌려준다.
 * 코드가 맞아도 detail 모양이 틀리거나 없으면 null이다(화면은 message만 보인다).
 */
export function fingerprintMismatch(err: unknown): FingerprintMismatchDetail | null {
  if (errorCode(err) !== "fingerprintMismatch") return null;
  const detail = errorDetail(err);
  if (typeof detail !== "object" || detail === null) return null;
  const { stored, actual } = detail as { stored?: unknown; actual?: unknown };
  if (!isFingerprintView(stored) || !isFingerprintView(actual)) return null;
  return { stored, actual };
}

/**
 * Tauri가 등록되지 않은 명령에 돌려주는 오류인가("Command disconnect_server not found").
 * 문자열로 오든 객체로 오든 본다. 새 명령이 아직 없는 옛 백엔드와 같이 쓸 때 조용히 넘기는 데 쓴다.
 */
export function isMissingCommand(err: unknown): boolean {
  const text = typeof err === "string" ? err : errorMessage(err);
  return /\bcommand\s+\S+\s+not found/i.test(text);
}

/**
 * 오류 문장을 제목(첫 문장)과 설명(나머지)으로 나눈다.
 * "미디어 연결에 실패했습니다. 방화벽에서 …" → 제목 "미디어 연결에 실패했습니다.", 설명 "방화벽에서 …".
 * 문장 끝은 마침표·느낌표·물음표 뒤에 공백이 오는 곳이라 "H.264"나 "192.168.0.1" 안의 점은 자르지 않는다.
 */
export function splitMessage(msg: string): { title: string; detail: string | null } {
  const text = msg.trim();
  const m = /^([\s\S]*?[.!?。])\s+([\s\S]+)$/.exec(text);
  if (!m) return { title: text, detail: null };
  return { title: m[1], detail: m[2].trim() };
}

/** 끝의 " (os error N)"(Rust가 붙이는 OS 오류 번호)를 지운다. 문장 앞뒤 공백도 정리한다. */
export function stripOsError(msg: string): string {
  return msg.replace(/\s*\(os error \d+\)\s*$/i, "").trim();
}

/** Windows 소켓 오류 번호별 접속 실패 이유. 모르는 번호는 문장에서 꺼낸다. */
const SOCKET_REASONS: Record<number, string> = {
  10061: "연결 거부",
  10060: "시간 초과",
  10065: "주소에 닿을 수 없음",
  10051: "주소에 닿을 수 없음",
};

/**
 * 접속 실패 문장을 홈 카드용으로 줄인다.
 * - `short`: 카드에 보이는 한 줄. "<주소>에 접속하지 못했습니다: …(os error N)"이면 번호로 "접속하지 못했습니다(연결 거부)"처럼,
 *   번호를 모르면 주소 접두사와 os error만 뺀 문장. 이미 "접속하지 못했습니다(시간 초과)" 꼴이면 주소만 뺀다.
 *   접속 실패 문장이 아니면 os error만 뺀다.
 * - `full`: 툴팁에 두는 전체 문장(os error만 뺀 것).
 */
export function connectFailure(msg: string): { short: string; full: string } {
  const full = stripOsError(msg);
  const m = /^\S+에 접속하지 못했습니다(\([^)]*\))?(?::\s*([\s\S]*))?$/.exec(full);
  if (!m) return { short: full, full };
  const [, paren, tail] = m;
  if (tail === undefined) return { short: `접속하지 못했습니다${paren ?? ""}`, full };
  const code = /\(os error (\d+)\)\s*$/i.exec(msg.trim())?.[1];
  const reason = code ? SOCKET_REASONS[Number(code)] : undefined;
  if (reason) return { short: `접속하지 못했습니다(${reason})`, full };
  return { short: tail.trim() || "접속하지 못했습니다", full };
}

/** 서버가 닫힐 때 이유가 따로 없으면 쓰는 기본 문구(`hub.rs`). */
export const DEFAULT_CLOSED_REASON = "서버와 연결이 끊어졌습니다";

function trimEnd(text: string): string {
  return text.trim().replace(/[.。\s]+$/, "");
}

/** 끊김 이유를 따로 보일 필요가 있는가: 기본 문구(끝의 마침표·공백 무시)거나 비어 있으면 null, 아니면 그대로. */
export function closedReasonDetail(reason: string): string | null {
  const t = trimEnd(reason);
  if (t === "" || t === trimEnd(DEFAULT_CLOSED_REASON)) return null;
  return reason;
}
