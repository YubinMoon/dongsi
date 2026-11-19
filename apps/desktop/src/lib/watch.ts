import { stripOsError } from "./errors";
import { codecLabel, fmt, qualityLabel } from "./format";
import { josa } from "./josa";
import type { LipSyncState } from "./lipsync";
import type { VideoCodec } from "./types";
import type { WatchStats } from "./watchStats";

/** 시청 화면의 상태. playing 이 아니면 영상 자리에 상태 화면이 보인다. */
export type WatchStatus = "connecting" | "playing" | "password" | "ended" | "error";

/** 방송 정보 중 표시에 쓰는 부분 */
export interface StreamInfo {
  codec: VideoCodec;
  width: number;
  height: number;
  fps: number;
}

export const MEDIA_FAILED_MESSAGE =
  "미디어 연결에 실패했습니다. 방화벽에서 UDP가 막혀 있는지 확인해 주세요.";
export const ENDED_MESSAGE = "방송이 끝났습니다.";
export const PASSWORD_REQUIRED_MESSAGE = "비밀번호를 입력해 주세요";
const CONNECT_FAILED_MESSAGE = "서버에 연결하지 못했습니다.";

// ---------- 이동 ----------

/** 돌아가기 대상: 내 방송(host) → 방송 화면, 로컬 감지(local-…) → 홈, 그 밖 → 서버 화면 */
export function backHref(key: string): string {
  if (key === "host") return "#/broadcast";
  if (key.startsWith("local-")) return "#/";
  return `#/server/${key}`;
}

/** 새 창으로 연 시청 화면인가: 주소 해시에 `popout=1`이 있다. */
export function isPopoutHash(hash: string): boolean {
  return /[?&]popout=1(?:&|$)/.test(hash);
}

// ---------- 표시 문자열 ----------

/** 위쪽 막대 오른쪽: "H.264 · 1920×1080 · 60fps" */
export function infoLine(info: StreamInfo): string {
  return `${codecLabel(info.codec)} · ${info.width}×${info.height} · ${info.fps}fps`;
}

export function viewersLabel(count: number): string {
  return `시청자 ${count}명`;
}

/** 비밀번호 폼 카드의 보조 줄: "도윤 · 4K · 30fps · AV1". 방송 정보가 없으면 이름만. */
export function passwordSubtitle(
  name: string,
  info: { codec: VideoCodec; height: number; fps: number } | null,
): string {
  if (!info) return name;
  return `${name} · ${qualityLabel(info.height, info.fps)} · ${codecLabel(info.codec)}`;
}

/** 끝남 화면의 설명. 조사는 이름의 받침에 맞춘다. */
export function endedDescription(name: string): string {
  const who = name.trim() || "방송자";
  return `${who}${josa(who, "이/가")} 다시 방송을 시작하면 자동으로 다시 연결합니다.`;
}

/** 비밀번호 입력란의 label */
export function passwordLabel(name: string): string {
  return `'${name}' 방송의 비밀번호`;
}

/** 서버 연결 실패 카드(`conn.card[key]`)에서 보여 줄 문장 */
export function cardFailureMessage(card: { kind: string; message?: string } | undefined): string {
  if (card && card.kind !== "connecting" && card.message) return stripOsError(card.message);
  return CONNECT_FAILED_MESSAGE;
}

// ---------- 상태 전이 ----------

export interface StartFailure {
  status: "password" | "ended" | "error";
  message: string;
  /** 비밀번호 폼 입력란 아래 오류 줄. null이면 보이지 않는다. */
  passwordError: string | null;
}

/**
 * 시청 시작(`watch_broadcast`)이 실패했을 때 다음 상태와 문구를 정한다.
 * - passwordRequired: 폼으로. 직접 입력해 보낸 뒤(submitted)에만 오류 줄을 보인다(처음 들어올 때는 폼 자체가 안내다).
 * - wrongPassword: 폼으로 돌아와 오류 줄을 보인다.
 * - notFound: 방송이 끝났다.
 * - 그 밖: 오류 화면(폼에서 보낸 경우도 같다).
 */
export function classifyStartError(
  err: { code: string | null; message: string },
  submitted: boolean,
): StartFailure {
  switch (err.code) {
    case "passwordRequired":
      return {
        status: "password",
        message: err.message,
        passwordError: submitted ? err.message : null,
      };
    case "wrongPassword":
      return { status: "password", message: err.message, passwordError: err.message };
    case "notFound":
      return { status: "ended", message: ENDED_MESSAGE, passwordError: null };
    default:
      return { status: "error", message: stripOsError(err.message), passwordError: null };
  }
}

/** RTCPeerConnection 연결 상태가 화면 상태를 바꾸는가. 바꾸지 않으면 null. */
export function statusFromConnectionState(
  state: string,
): { status: "playing" } | { status: "error"; message: string } | null {
  if (state === "connected") return { status: "playing" };
  if (state === "failed") return { status: "error", message: MEDIA_FAILED_MESSAGE };
  return null;
}

/** 끝났거나 끊긴 상태에서만 같은 방송자가 다시 방송하면 다시 접속한다. */
export function shouldResume(status: WatchStatus): boolean {
  return status === "ended" || status === "error";
}

/** 왼쪽 위 막대의 LIVE 배지: 방송 정보가 있고 끝나지 않았을 때 */
export function showLive(status: WatchStatus, hasInfo: boolean): boolean {
  return hasInfo && status !== "ended";
}

/** 오른쪽 위 방송 정보: 방송 정보가 있고, 끝남·비밀번호 상태가 아닐 때(시안) */
export function showInfoBar(status: WatchStatus, hasInfo: boolean): boolean {
  return hasInfo && status !== "ended" && status !== "password";
}

/** 입력 중인 요소에서는 F·M 키를 가로채지 않는다. 음량 슬라이더는 예외. */
export function isTypingTarget(tagName: string, inputType: string | null): boolean {
  const tag = tagName.toLowerCase();
  if (tag === "textarea" || tag === "select") return true;
  if (tag !== "input") return false;
  return inputType !== "range" && inputType !== "checkbox";
}

// ---------- 음소거 ----------

export interface MuteState {
  /** 사용자가 소리를 끈 상태 */
  muted: boolean;
  /** 자동 재생 정책 때문에 소리를 끄고 재생 중인 상태(사용자가 켜야 한다) */
  needsUnmute: boolean;
}

/**
 * 음소거 단추·M 키를 한 번 눌렀을 때의 전이.
 * 소리가 막힌 상태(needsUnmute)나 꺼진 상태에서는 바로 켠다(unmute: true — 호출한 쪽이 video.muted를 풀고 재생을 다시 시도한다).
 * 켜진 상태에서는 끈다.
 */
export function toggleMute(state: MuteState): MuteState & { unmute: boolean } {
  if (state.needsUnmute || state.muted) {
    return { muted: false, needsUnmute: false, unmute: true };
  }
  return { muted: true, needsUnmute: false, unmute: false };
}

/** 영상 요소에 실제로 적용할 음소거 */
export function effectiveMuted(state: MuteState): boolean {
  return state.muted || state.needsUnmute;
}

/** 음량 단추의 모양: 소리가 안 나는 상태면 volume-x */
export function volumeIconName(state: MuteState): "volume" | "volume-x" {
  return effectiveMuted(state) ? "volume-x" : "volume";
}

/** 음량 단추의 aria-label */
export function muteButtonLabel(state: MuteState): string {
  return effectiveMuted(state) ? "소리 켜기 (M)" : "음소거 (M)";
}

// ---------- 멈춤 감지 ----------

export interface FreezeState {
  lastFramesDecoded: number;
  /** 프레임이 늘지 않기 시작한 시각(ms). 늘고 있으면 null */
  frozenSince: number | null;
  lastKeyframeRequest: number;
}

export const FREEZE_INITIAL: FreezeState = {
  lastFramesDecoded: 0,
  frozenSince: null,
  lastKeyframeRequest: 0,
};

const FREEZE_AFTER_MS = 2000;
const KEYFRAME_RETRY_MS = 2000;

/**
 * 화면이 멈췄는데 패킷은 오면 키프레임을 요청한다(브라우저도 PLI를 보내지만 한 번 더).
 * 주기적으로 부른다(시청 페이지의 통계 루프라 처음 15초는 0.5초, 그 뒤는 1초 간격). 시간은 `now`로 재므로 호출 간격이 달라도 동작은 같다.
 * 재생 중이 아니면 멈춤으로 세지 않는다.
 */
export function checkFrozen(
  state: FreezeState,
  input: { framesDecoded: number; videoKbps: number; playing: boolean; now: number },
): { state: FreezeState; requestKeyframe: boolean } {
  if (input.framesDecoded > state.lastFramesDecoded) {
    return {
      state: { ...state, lastFramesDecoded: input.framesDecoded, frozenSince: null },
      requestKeyframe: false,
    };
  }
  if (!input.playing || !(input.videoKbps > 0)) {
    return { state, requestKeyframe: false };
  }
  const frozenSince = state.frozenSince ?? input.now;
  const request =
    input.now - frozenSince > FREEZE_AFTER_MS && input.now - state.lastKeyframeRequest > KEYFRAME_RETRY_MS;
  return {
    state: { ...state, frozenSince, lastKeyframeRequest: request ? input.now : state.lastKeyframeRequest },
    requestKeyframe: request,
  };
}

// ---------- 통계 ----------

export interface StatRow {
  label: string;
  value: string;
}

/** 립싱크 보정의 지금 상태. enabled가 false면 보정기가 꺼져 있다(개발 빌드의 측정용 스위치). */
export interface LipSyncView {
  enabled: boolean;
  /** 양수면 비디오 바닥, 음수면 오디오 바닥(ms) */
  adjustMs: number;
  /** 보정기 상태(개발 빌드). 없으면 안정으로 본다. 불안정·회복 중이면 줄 끝에 " · 보류"가 붙는다. */
  state?: LipSyncState;
}

/** 통계 패널 "립싱크 보정" 줄의 값: "비디오 +120ms" / "오디오 +30ms" / "0ms" / "꺼짐". 불안정·회복 중이면 " · 보류"가 붙는다. */
export function lipsyncLabel(view: LipSyncView): string {
  if (!view.enabled) return "꺼짐";
  const ms = Math.round(view.adjustMs);
  const label = ms > 0 ? `비디오 +${ms}ms` : ms < 0 ? `오디오 +${-ms}ms` : "0ms";
  return view.state && view.state !== "stable" ? `${label} · 보류` : label;
}

/**
 * 통계 패널의 줄. 개발 빌드(dev)에서는 목표 지연, (lipsync가 있으면) 립싱크 보정, 스트림 ID 줄이 더 붙는다.
 * 배포 빌드의 줄은 바뀌지 않는다.
 */
export function statRows(
  stats: WatchStats,
  opts: { dev: boolean; streamIds: string[]; lipsync?: LipSyncView | null },
): StatRow[] {
  const rows: StatRow[] = [
    { label: "해상도", value: `${stats.width}×${stats.height} · ${fmt(stats.fps)}fps` },
    { label: "코덱", value: `${stats.codec ?? "–"}${stats.decoder ? ` (${stats.decoder})` : ""}` },
    { label: "비트레이트", value: `${fmt(stats.videoKbps)} + ${fmt(stats.audioKbps)} kbps` },
    { label: "지연", value: `${fmt(stats.latencyMs)} ms` },
    { label: "A/V 차이", value: `${fmt(stats.avOffsetMs)} ms` },
    { label: "지터 버퍼", value: `${fmt(stats.videoJitterMs)} / ${fmt(stats.audioJitterMs)} ms` },
    { label: "RTT", value: `${fmt(stats.rttMs)} ms` },
    {
      label: "손실",
      value: `${stats.packetsLost} · 버린 프레임 ${stats.framesDropped} · 멈춤 ${stats.freezeCount}`,
    },
  ];
  if (opts.dev) {
    rows.push({ label: "목표 지연", value: `${fmt(stats.videoTargetMs)} / ${fmt(stats.audioTargetMs)} ms` });
    if (opts.lipsync) rows.push({ label: "립싱크 보정", value: lipsyncLabel(opts.lipsync) });
    rows.push({ label: "스트림", value: opts.streamIds.join(" ") });
  }
  return rows;
}

/** 개발 빌드가 2초마다 앱 로그에 남기는 한 줄 */
export function devLogLine(stats: WatchStats, hidden: boolean): string {
  const r = (n: number | null) => (n === null ? "-" : n.toFixed(0));
  return (
    `watch ${stats.width}x${stats.height} fps=${r(stats.fps)} kbps=${r(stats.videoKbps)}+${r(stats.audioKbps)} ` +
    `latency=${r(stats.latencyMs)} av=${r(stats.avOffsetMs)} jb=${r(stats.videoJitterMs)}/${r(stats.audioJitterMs)} ` +
    `target=${r(stats.videoTargetMs)}/${r(stats.audioTargetMs)} lost=${stats.packetsLost} dropped=${stats.framesDropped} ` +
    `freeze=${stats.freezeCount} codec=${stats.codec} hidden=${hidden}`
  );
}
