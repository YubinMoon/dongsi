import { describe, expect, it } from "vitest";
import {
  audioModes,
  audioNote,
  codecOptions,
  codecUsable,
  discordRow,
  encoderLine,
  firstUsableCodec,
  groupSources,
  hostingHint,
  levelRatio,
  levelText,
  normalizeBitrate,
  pickSourceId,
  presetIdFor,
  previewMeta,
  receivableFrom,
  sendRows,
  settingsFor,
  setupSummary,
  shareLine,
  shareNotices,
  sourceSub,
  startError,
  videoRows,
} from "./broadcast";
import type { BroadcastStatus, EncoderView, PresetView, SourceView } from "./types";

const encoders: EncoderView[] = [
  { codec: "h264", name: "h264_nvenc", hardware: true },
  { codec: "vp9", name: "libvpx-vp9", hardware: false },
];
const all = new Set(["video/h264", "video/vp8", "video/vp9", "video/av1"]);

const preset = (id: string, w: number, h: number, fps: number, label = id): PresetView => ({
  id,
  label,
  width: w,
  height: h,
  fps,
  bitrates: { h264: 7000, h265: 5600, vp8: 7000, vp9: 5600, av1: 4900 },
});
const presets = [
  preset("p720p30", 1280, 720, 30, "720p 30fps"),
  preset("p1080p60", 1920, 1080, 60, "1080p 60fps"),
  preset("source", 0, 0, 60, "원본 60fps"),
];

const src = (over: Partial<SourceView>): SourceView => ({
  id: "x",
  kind: "monitor",
  name: "화면 1",
  width: 3840,
  height: 2160,
  primary: false,
  processName: null,
  ...over,
});

describe("codecUsable", () => {
  it("인코더 목록이 아직 없으면(null) 인코더 쪽은 막지 않는다", () => {
    expect(codecUsable("h264", null, all)).toEqual({ ok: true, reason: null, encoder: "" });
  });
  it("인코더가 없으면 '인코더 없음'", () => {
    expect(codecUsable("av1", encoders, all)).toEqual({ ok: false, reason: "인코더 없음", encoder: "" });
  });
  it("시청 앱이 못 받으면 '재생 불가'", () => {
    const r = codecUsable("vp9", encoders, new Set(["video/h264"]));
    expect(r).toEqual({ ok: false, reason: "재생 불가", encoder: "" });
  });
  it("인코더 없음이 재생 불가보다 먼저다", () => {
    expect(codecUsable("h265", encoders, new Set(["video/h264"])).reason).toBe("인코더 없음");
  });
  it("재생 가능 목록을 못 읽었으면(빈 집합) 재생 쪽은 막지 않는다", () => {
    expect(codecUsable("h264", encoders, new Set()).ok).toBe(true);
  });
  it("쓸 수 있으면 인코더 설명(하드웨어 표시)", () => {
    expect(codecUsable("h264", encoders, all).encoder).toBe("h264_nvenc · 하드웨어");
    expect(codecUsable("vp9", encoders, all).encoder).toBe("libvpx-vp9");
  });
});

describe("firstUsableCodec", () => {
  it("지금 코덱이 쓸 수 있으면 그대로", () => {
    expect(firstUsableCodec("vp9", encoders, all)).toBe("vp9");
  });
  it("못 쓰면 순서상 첫 번째로", () => {
    expect(firstUsableCodec("av1", encoders, all)).toBe("h264");
    expect(firstUsableCodec("h264", encoders, new Set(["video/vp9"]))).toBe("vp9");
  });
  it("하나도 없으면 그대로", () => {
    expect(firstUsableCodec("av1", [], all)).toBe("av1");
  });
});

describe("codecOptions", () => {
  it("다섯 코덱을 순서대로, 못 쓰는 것은 disabled와 이유", () => {
    const o = codecOptions(encoders, new Set(["video/h264", "video/vp9"]));
    expect(o.map((x) => x.label)).toEqual(["H.264", "H.265", "VP8", "VP9", "AV1"]);
    expect(o.map((x) => x.disabled)).toEqual([false, true, true, false, true]);
    expect(o[1].reason).toBe("인코더 없음");
    expect(o[0].reason).toBeUndefined();
  });
});

describe("receivableFrom", () => {
  it("mimeType을 소문자로 모은다", () => {
    expect([...receivableFrom({ codecs: [{ mimeType: "video/H264" }, { mimeType: "video/VP9" }] })]).toEqual([
      "video/h264",
      "video/vp9",
    ]);
  });
  it("없으면 빈 집합", () => {
    expect(receivableFrom(null).size).toBe(0);
    expect(receivableFrom(undefined).size).toBe(0);
  });
});

describe("encoderLine", () => {
  const monitor = src({ width: 2560, height: 1440 });
  it("확인 중", () => {
    expect(encoderLine("h264", null, all, presets[1], monitor)).toBe("인코더를 확인하는 중…");
  });
  it("인코더 · 출력 최대 (원본)", () => {
    expect(encoderLine("h264", encoders, all, presets[1], monitor)).toBe(
      "h264_nvenc · 하드웨어 · 출력 최대 1920×1080 (원본 2560×1440)",
    );
  });
  it("원본 프리셋(너비 0)은 출력 최대를 쓰지 않는다", () => {
    expect(encoderLine("h264", encoders, all, presets[2], monitor)).toBe("h264_nvenc · 하드웨어");
  });
  it("소스가 없으면 인코더만", () => {
    expect(encoderLine("h264", encoders, all, presets[1], null)).toBe("h264_nvenc · 하드웨어");
  });
  it("못 쓰는 코덱이면 이유", () => {
    expect(encoderLine("av1", encoders, all, presets[1], null)).toBe("인코더 없음");
  });
});

describe("groupSources / sourceSub / pickSourceId", () => {
  const list = [
    src({ id: "w1", kind: "window", name: "Chrome", processName: "chrome.exe", width: 1920, height: 1040 }),
    src({ id: "m2", name: "화면 2", width: 1920, height: 1080 }),
    src({ id: "m1", name: "화면 1", primary: true }),
    src({ id: "t", kind: "test", name: "테스트 패턴", width: 1920, height: 1080 }),
  ];
  it("화면, 창, 테스트 순이고 빈 묶음은 뺀다", () => {
    expect(groupSources(list).map((g) => [g.label, g.items.map((s) => s.id)])).toEqual([
      ["화면", ["m2", "m1"]],
      ["창", ["w1"]],
      ["테스트", ["t"]],
    ]);
    expect(groupSources(list.slice(0, 1)).map((g) => g.label)).toEqual(["창"]);
    expect(groupSources([])).toEqual([]);
  });
  it("보조 줄", () => {
    expect(sourceSub(list[2])).toBe("주 화면 · 3840×2160");
    expect(sourceSub(list[1])).toBe("화면 · 1920×1080");
    expect(sourceSub(list[0])).toBe("chrome.exe · 1920×1040");
    expect(sourceSub({ ...list[0], processName: null })).toBe("창 · 1920×1040");
    expect(sourceSub(list[3])).toBe("개발용 · 1920×1080");
  });
  it("지금 소스가 남아 있으면 유지, 아니면 주 화면, 아니면 첫 번째", () => {
    expect(pickSourceId(list, "w1")).toBe("w1");
    expect(pickSourceId(list, "gone")).toBe("m1");
    expect(pickSourceId(list.filter((s) => !s.primary), "gone")).toBe("w1");
    expect(pickSourceId([], "gone")).toBe("");
  });
});

describe("audio", () => {
  it("테스트 톤은 개발 빌드에서만", () => {
    expect(audioModes(false)).toEqual(["systemExceptDiscord", "app", "system", "none"]);
    expect(audioModes(true)).toContain("testTone");
  });
  it("모드별 안내의 색", () => {
    expect(audioNote("systemExceptDiscord").tone).toBe("ok");
    expect(audioNote("systemExceptDiscord").text).toBe(
      "PC에서 나는 소리를 보내되, Discord 소리는 방송에 넣지 않습니다.",
    );
    expect(audioNote("system")).toMatchObject({ tone: "warn", text: "Discord 소리도 방송에 들어갑니다." });
    expect(audioNote("app").text).toBe("캡처하는 앱의 소리만 보냅니다.");
    expect(audioNote("none").text).toBe("소리 없이 화면만 보냅니다.");
    expect(audioNote("testTone").tone).toBe("info");
  });
});

describe("hostingHint / setupSummary / startError", () => {
  it("방식별 설명, 서버가 없으면 안내", () => {
    expect(hostingHint("lan", 0)).toContain("이 PC가 서버가 됩니다");
    expect(hostingHint("direct", 0)).toContain("UPnP");
    expect(hostingHint("server", 2)).toContain("전용 서버에서 방송합니다");
    expect(hostingHint("server", 0)).toBe("추가한 서버가 없습니다.");
  });
  it("요약 줄", () => {
    expect(setupSummary(presets[1], "h264", "lan", null)).toBe("1080p 60fps · H.264 · 로컬 랜");
    expect(setupSummary(presets[1], "av1", "direct", null)).toBe("1080p 60fps · AV1 · PC 직접 호스팅");
    expect(setupSummary(presets[1], "h264", "server", "우리집 서버")).toBe("1080p 60fps · H.264 · 우리집 서버");
    expect(setupSummary(presets[1], "h264", "server", null)).toBe("1080p 60fps · H.264 · 추가한 서버");
    expect(setupSummary(null, "h264", "lan", null)).toBe("H.264 · 로컬 랜");
  });
  it("시작 전 오류", () => {
    expect(startError("", "lan", "")).toBe("캡처할 화면이나 창을 골라 주세요");
    expect(startError("m1", "server", "")).toBe("방송할 서버를 골라 주세요");
    expect(startError("m1", "server", "s1")).toBeNull();
    expect(startError("m1", "lan", "")).toBeNull();
  });
});

describe("비트레이트·설정", () => {
  it("normalizeBitrate", () => {
    expect(normalizeBitrate(7000.4, 1)).toBe(7000);
    expect(normalizeBitrate(null, 5000)).toBe(5000);
    expect(normalizeBitrate(Number.NaN, 5000)).toBe(5000);
    expect(normalizeBitrate(0, 5000)).toBe(5000);
    expect(normalizeBitrate(-3, 5000)).toBe(5000);
  });
  it("settingsFor", () => {
    expect(settingsFor(presets[0], "vp9", 3000)).toEqual({
      codec: "vp9",
      width: 1280,
      height: 720,
      fps: 30,
      bitrateKbps: 3000,
    });
    expect(settingsFor(null, "h264", 7000)).toMatchObject({ width: 1920, height: 1080, fps: 60 });
    expect(settingsFor(presets[2], "h264", 7000)).toMatchObject({ width: 0, height: 0, fps: 60 });
  });
  it("presetIdFor", () => {
    expect(presetIdFor(presets, { codec: "h264", width: 1920, height: 1080, fps: 60, bitrateKbps: 1 })).toBe("p1080p60");
    expect(presetIdFor(presets, { codec: "h264", width: 0, height: 0, fps: 60, bitrateKbps: 1 })).toBe("source");
    expect(presetIdFor(presets, { codec: "h264", width: 5, height: 5, fps: 5, bitrateKbps: 1 })).toBe("p720p30");
    expect(presetIdFor([], { codec: "h264", width: 5, height: 5, fps: 5, bitrateKbps: 1 })).toBe("");
  });
});

describe("공유 안내", () => {
  it("일부러 고른 포트(요청 8450 = 실제 8450)는 대체가 아니라 안내가 없다", () => {
    expect(shareNotices({ port: 8450, requestedPort: 8450, portFallback: false, upnp: null })).toEqual([]);
    expect(shareNotices({ port: 7885, requestedPort: 7885, portFallback: false, upnp: null })).toEqual([]);
  });
  it("대체: 문구에 요청한 포트와 실제 포트가 들어간다", () => {
    const n = shareNotices({ port: 51000, requestedPort: 8450, portFallback: true, upnp: null });
    expect(n).toHaveLength(1);
    expect(n[0].id).toBe("port");
    expect(n[0].text).toContain("8450번 포트를 다른 프로그램");
    expect(n[0].text).toContain("51000번으로 열었습니다");
    expect(n[0].text).not.toContain("7884");
  });
  it("전용 서버 방송(포트 없음)은 안내 없음", () => {
    expect(shareNotices({ port: null, requestedPort: null, portFallback: false, upnp: null })).toEqual([]);
  });
  it("안내 없음", () => {
    expect(shareNotices({ port: 7884, requestedPort: 7884, portFallback: false, upnp: null })).toEqual([]);
    expect(shareNotices({ port: null, requestedPort: null, portFallback: false, upnp: null })).toEqual([]);
  });
  it("포트 대체 → UPnP 열림 → 이중 NAT 순서", () => {
    const n = shareNotices({ port: 7885, requestedPort: 7884, portFallback: true, upnp: { external: "100.64.12.7:7885", doubleNat: true, error: null } });
    expect(n.map((x) => x.id)).toEqual(["port", "upnp", "doubleNat"]);
    expect(n[0].tone).toBe("warn");
    expect(n[0].text).toContain("7884번 포트를 다른 프로그램");
    expect(n[0].text).toContain("7885번으로 열었습니다");
    expect(n[1]).toMatchObject({ tone: "ok", text: "UPnP 포트 열림: ", code: "100.64.12.7:7885" });
    expect(n[2].tone).toBe("warn");
  });
  it("UPnP 열림이 정상이면 이중 NAT 안내 없음", () => {
    const n = shareNotices({ port: 7884, requestedPort: 7884, portFallback: false, upnp: { external: "203.0.113.7:7884", doubleNat: false, error: null } });
    expect(n.map((x) => x.id)).toEqual(["upnp"]);
  });
  it("UPnP 오류는 호박 안내, 외부 주소가 있으면 오류는 숨김", () => {
    expect(shareNotices({ port: 7884, requestedPort: 7884, portFallback: false, upnp: { external: null, doubleNat: false, error: "게이트웨이를 못 찾음" } })).toEqual([
      { id: "upnpError", tone: "warn", text: "게이트웨이를 못 찾음" },
    ]);
    expect(
      shareNotices({ port: 7884, requestedPort: 7884, portFallback: false, upnp: { external: "1.2.3.4:7884", doubleNat: false, error: "무시" } }).map((x) => x.id),
    ).toEqual(["upnp"]);
    expect(shareNotices({ port: 7884, requestedPort: 7884, portFallback: false, upnp: { external: null, doubleNat: false, error: null } })).toEqual([]);
  });
  it("shareLine", () => {
    expect(shareLine({ hosting: "direct", serverName: "지우", port: 7884 })).toBe("PC 직접 호스팅 · 지우 · 포트 7884");
    expect(shareLine({ hosting: "server", serverName: "우리집 서버", port: null })).toBe("추가한 서버 · 우리집 서버");
  });
});

describe("음량", () => {
  it("levelRatio", () => {
    expect(levelRatio(-18)).toBeCloseTo(0.7);
    expect(levelRatio(0)).toBe(1);
    expect(levelRatio(6)).toBe(1);
    expect(levelRatio(-60)).toBe(0);
    expect(levelRatio(-120)).toBe(0);
    expect(levelRatio(Number.NEGATIVE_INFINITY)).toBe(0);
    expect(levelRatio(null)).toBe(0);
  });
  it("levelText", () => {
    expect(levelText(-18.2)).toBe("-18 dBFS");
    expect(levelText(null)).toBe("–");
    expect(levelText(Number.NaN)).toBe("–");
  });
});

describe("상태 줄", () => {
  const status = {
    sourceName: "ELDEN RING™",
    width: 1920,
    height: 1080,
    settings: { codec: "h264", width: 1920, height: 1080, fps: 60, bitrateKbps: 7000 },
    video: {
      encoder: "h264_nvenc",
      hardware: true,
      codec: "h264",
      width: 1920,
      height: 1080,
      sourceWidth: 2560,
      sourceHeight: 1440,
      captureFps: 60,
      encodeFps: 59.6,
      avgEncodeMs: 2.14,
      bitrateKbps: 7000,
      droppedFrames: 1,
      avgConvertMs: 0.42,
      avgLatencyMs: 12.34,
      gpuConvert: true,
      frames: 100,
      keyframes: 1,
    },
    publish: {
      connected: true,
      closed: null,
      videoKbps: 6950,
      audioKbps: 128,
      rttMs: 4,
      loss: 0.0123,
      nacks: 0,
      keyframeRequests: 1,
      droppedFrames: 2,
    },
  } as unknown as BroadcastStatus;

  it("previewMeta", () => {
    expect(previewMeta(status)).toBe("ELDEN RING™ · 1920×1080 · 60fps · H.264");
  });
  it("videoRows", () => {
    const rows = Object.fromEntries(videoRows(status).map((r) => [r.label, r.value]));
    expect(rows["캡처"]).toBe("ELDEN RING™");
    expect(rows["출력"]).toBe("1920×1080 · H.264");
    expect(rows["인코더"]).toBe("h264_nvenc (하드웨어)");
    expect(rows["프레임"]).toBe("캡처 60 / 인코딩 60 fps · 버림 3");
    expect(rows["인코딩"]).toBe("평균 2.1 ms · 변환 0.4 ms (GPU)");
    expect(rows["캡처→출력"]).toBe("12.3 ms");
  });
  it("인코더 이름이 없고 소프트웨어면", () => {
    const rows = videoRows({ ...status, video: { ...status.video, encoder: "", hardware: false, gpuConvert: false } });
    expect(rows.find((r) => r.label === "인코더")?.value).toBe("– (소프트웨어)");
    expect(rows.find((r) => r.label === "인코딩")?.value).toContain("(CPU)");
  });
  it("sendRows", () => {
    expect(sendRows(status.publish).map((r) => r.label + "=" + r.value)).toEqual([
      "비디오=6950 kbps",
      "오디오=128 kbps",
      "RTT=4 ms",
      "손실=1.2%",
      "키프레임 요청=1",
    ]);
    expect(sendRows({ ...status.publish, rttMs: null, loss: null }).map((r) => r.value)).toContain("– ms");
    expect(sendRows({ ...status.publish, loss: null })[3].value).toBe("0.0%");
  });
  it("discordRow는 Discord 제외 모드에서만", () => {
    expect(discordRow("systemExceptDiscord", { discordExcluded: true })).toEqual({
      label: "Discord",
      value: "Discord 소리 빼는 중",
      tone: "ok",
    });
    expect(discordRow("systemExceptDiscord", { discordExcluded: false })?.value).toBe(
      "Discord가 실행 중이 아님(켜지면 자동으로 뺌)",
    );
    expect(discordRow("system", { discordExcluded: false })).toBeNull();
    expect(discordRow("systemExceptDiscord", null)).toBeNull();
  });
});
