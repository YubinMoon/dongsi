import type { ViewerChime } from "./viewerChime";

// 화면·Discord 창 방송에서는 이 소리도 스트림 오디오로 송출된다(Discord 트리만 EXCLUDE). 사용자 결정으로 받아들이고, 대신 짧고 작게 낸다.

/** 두 음(Hz). 입장은 오르고 퇴장은 내린다. */
const NOTES: Record<ViewerChime, readonly number[]> = { join: [660, 880], leave: [880, 660] };
/** 음 하나의 길이. 두 음이라 전체는 250ms다. */
const NOTE_SECONDS = 0.125;
const ATTACK_SECONDS = 0.01;
/** 게임 소리를 가리지 않을 크기 */
const PEAK_GAIN = 0.09;

/** 처음 소리를 낼 때 하나만 만든다. */
let context: AudioContext | null = null;

/** 짧은 sine 음 하나. 클릭이 나지 않게 0에서 올렸다가 지수 감쇠로 줄인다. */
function playNote(ctx: AudioContext, hz: number, start: number) {
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.frequency.value = hz;
  gain.gain.setValueAtTime(0, start);
  gain.gain.linearRampToValueAtTime(PEAK_GAIN, start + ATTACK_SECONDS);
  // 지수 감쇠는 0까지 갈 수 없어 들리지 않는 크기까지만 줄인다.
  gain.gain.exponentialRampToValueAtTime(0.001, start + NOTE_SECONDS);
  osc.connect(gain).connect(ctx.destination);
  osc.start(start);
  osc.stop(start + NOTE_SECONDS);
}

async function play(kind: ViewerChime) {
  context ??= new AudioContext();
  if (context.state === "suspended") await context.resume();
  const start = context.currentTime;
  for (const [i, hz] of NOTES[kind].entries()) playNote(context, hz, start + i * NOTE_SECONDS);
}

/**
 * 내 방송의 시청자 입장·퇴장 알림음을 낸다.
 * 소리를 내지 못해도 방송에는 지장이 없으므로 경고만 남긴다.
 */
export function playViewerChime(kind: ViewerChime): void {
  play(kind).catch((e) => console.warn("알림음을 내지 못했습니다", e));
}
