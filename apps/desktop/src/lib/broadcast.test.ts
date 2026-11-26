import { describe, expect, it } from "vitest";
import {
  appAudioAllowed,
  audioForSource,
  canSwitchSource,
  followNote,
  discordRow,
  encoderBlock,
  encoderLine,
  groupSources,
  hostingHint,
  levelRatio,
  levelText,
  fpsHint,
  LatestOnly,
  liveSource,
  mergeThumbnails,
  outputSize,
  pickSourceId,
  qualityTarget,
  resolvedFps,
  resolutionValue,
  RESOLUTIONS,
  FRAME_RATES,
  networkLimitHint,
  NO_ENCODER_MESSAGE,
  normalizeAudioBoost,
  sendRows,
  settingsFor,
  setupSummary,
  shareLine,
  shareNotices,
  sourceSub,
  startError,
  switchableSources,
  switchNotes,
  thumbnailIds,
  videoRows,
} from "./broadcast";
import type { BroadcastStatus, SourceThumbnail, SourceView } from "./types";

const fhd60 = { width: 1920, height: 1080, fps: 60 };

const src = (over: Partial<SourceView>): SourceView => ({
  id: "x",
  kind: "monitor",
  name: "화면 1",
  width: 3840,
  height: 2160,
  primary: false,
  processName: null,
  discord: false,
  follow: null,
  refreshRate: 60,
  ...over,
});

describe("encoderBlock(D-103: 하드웨어 H.264 인코더가 없으면 방송할 수 없다)", () => {
  it("없으면(null) 시작을 막고 백엔드와 같은 문구로 알린다", () => {
    expect(encoderBlock(null)).toBe(NO_ENCODER_MESSAGE);
    expect(NO_ENCODER_MESSAGE).toBe("하드웨어 H.264 인코더(NVIDIA·AMD·Intel)를 찾지 못해 방송할 수 없습니다");
  });
  it("확인하는 중(undefined)에는 막지 않는다", () => {
    expect(encoderBlock(undefined)).toBeNull();
  });
  it("인코더가 있으면 막지 않는다", () => {
    expect(encoderBlock("h264_nvenc")).toBeNull();
    expect(encoderBlock("h264_amf")).toBeNull();
    expect(encoderBlock("h264_qsv")).toBeNull();
  });
});

describe("encoderLine", () => {
  const monitor = src({ width: 2560, height: 1440 });
  const scaled = { width: 1920, height: 1080, fps: 60 };
  const same = { width: 2560, height: 1440, fps: 60 };
  it("확인 중", () => {
    expect(encoderLine(undefined, scaled, monitor)).toBe("인코더를 확인하는 중…");
  });
  it("인코더 · 출력 (원본)", () => {
    expect(encoderLine("h264_nvenc", scaled, monitor)).toBe("h264_nvenc · 출력 1920×1080 (원본 2560×1440)");
  });
  it("출력이 원본과 같으면 원본 크기를 다시 쓰지 않는다", () => {
    expect(encoderLine("h264_amf", same, monitor)).toBe("h264_amf · 출력 2560×1440");
  });
  it("소스가 없으면 인코더만", () => {
    expect(encoderLine("h264_nvenc", scaled, null)).toBe("h264_nvenc");
  });
  it("인코더가 없으면 짧은 안내만(출력 크기는 쓰지 않는다)", () => {
    expect(encoderLine(null, scaled, monitor)).toBe("하드웨어 H.264 인코더 없음");
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

describe("hostingHint / setupSummary / startError", () => {
  it("방식별 설명, 서버가 없으면 안내", () => {
    expect(hostingHint("lan", 0)).toContain("이 PC가 서버가 됩니다");
    expect(hostingHint("direct", 0)).toContain("UPnP");
    expect(hostingHint("server", 2)).toContain("전용 서버에서 방송합니다");
    expect(hostingHint("server", 0)).toBe("추가한 서버가 없습니다.");
  });
  it("요약 줄: 실제로 나갈 크기·fps", () => {
    expect(setupSummary(fhd60, "lan", null)).toBe("1920×1080 60fps · 로컬 랜");
    expect(setupSummary({ width: 2560, height: 1440, fps: 30 }, "direct", null)).toBe("2560×1440 30fps · PC 직접 호스팅");
    expect(setupSummary(fhd60, "server", "우리집 서버")).toBe("1920×1080 60fps · 우리집 서버");
    expect(setupSummary(fhd60, "server", null)).toBe("1920×1080 60fps · 추가한 서버");
    expect(setupSummary(null, "lan", null)).toBe("로컬 랜");
  });
  it("시작 전 오류", () => {
    expect(startError("", "lan", "")).toBe("캡처할 화면이나 창을 골라 주세요");
    expect(startError("m1", "server", "")).toBe("방송할 서버를 골라 주세요");
    expect(startError("m1", "server", "s1")).toBeNull();
    expect(startError("m1", "lan", "")).toBeNull();
  });
});

describe("해상도·주사율(D-074)", () => {
  it("칩: 해상도 FHD·QHD(원본 없음, D-097), 주사율 30·60(144·원본 없음, D-101)", () => {
    expect(RESOLUTIONS.map((r) => [r.label, r.width, r.height])).toEqual([
      ["FHD", 1920, 1080],
      ["QHD", 2560, 1440],
    ]);
    expect(FRAME_RATES.map((f) => [f.label, f.fps])).toEqual([
      ["30", 30],
      ["60", 60],
    ]);
    expect(resolutionValue(1920, 1080)).toBe(RESOLUTIONS[0].value);
  });

  it("outputSize는 Rust VideoSettings::output_size와 같다(비율 유지, 원본이 작으면 원본, 짝수)", () => {
    // crates/core/src/media.rs output_size_keeps_aspect_and_even 과 같은 값
    expect(outputSize(1920, 1080, 3840, 2160)).toEqual({ width: 1920, height: 1080 });
    expect(outputSize(1920, 1080, 1280, 720)).toEqual({ width: 1280, height: 720 });
    expect(outputSize(1920, 1080, 3440, 1440)).toEqual({ width: 1920, height: 804 });
    expect(outputSize(1920, 1080, 1848, 1156)).toEqual({ width: 1726, height: 1080 });
    expect(outputSize(0, 0, 1921, 1081)).toEqual({ width: 1920, height: 1080 });
    expect(outputSize(2560, 1440, 3840, 2160)).toEqual({ width: 2560, height: 1440 });
    // 4K는 없다(D-097): 상자 0이나 더 큰 상자도 2560×1440 안으로.
    expect(outputSize(0, 0, 3840, 2160)).toEqual({ width: 2560, height: 1440 });
    expect(outputSize(3840, 2160, 3840, 2160)).toEqual({ width: 2560, height: 1440 });
    expect(outputSize(0, 0, 3440, 1440)).toEqual({ width: 2560, height: 1072 });
    expect(outputSize(0, 720, 3840, 2160)).toEqual({ width: 1280, height: 720 });
    expect(outputSize(1920, 1080, 1, 1)).toEqual({ width: 2, height: 2 });
  });

  it("resolvedFps: 30·60만(다른 값은 60, D-107), 모니터보다 높으면 모니터까지", () => {
    expect(resolvedFps(60, 144)).toBe(60);
    expect(resolvedFps(30, 144)).toBe(30);
    expect(resolvedFps(60, null)).toBe(60);
    expect(resolvedFps(60, 59.94)).toBe(60);
    expect(resolvedFps(60, 30)).toBe(30);
    // 30·60이 아닌 값(옛 "원본" 0, 144)은 Rust `VideoSettings::clamped`처럼 60
    expect(resolvedFps(0, 144)).toBe(60);
    expect(resolvedFps(144, null)).toBe(60);
    expect(resolvedFps(144, 30)).toBe(30);
  });

  it("qualityTarget: 대상 크기와 모니터 주사율로", () => {
    const monitor = src({ width: 2560, height: 1440, refreshRate: 165 });
    expect(qualityTarget({ width: 1920, height: 1080, fps: 60 }, monitor)).toEqual(fhd60);
    expect(qualityTarget({ width: 0, height: 0, fps: 30 }, monitor)).toEqual({
      width: 2560,
      height: 1440,
      fps: 30,
    });
    // 모니터가 30Hz면 60fps를 골라도 30fps까지만
    expect(qualityTarget({ width: 1920, height: 1080, fps: 60 }, src({ width: 1920, height: 1080, refreshRate: 30 }))).toEqual({
      width: 1920,
      height: 1080,
      fps: 30,
    });
    // 대상을 아직 모르면: 원본 해상도는 FHD로 본다
    expect(qualityTarget({ width: 0, height: 0, fps: 60 }, null)).toEqual(fhd60);
    expect(qualityTarget({ width: 2560, height: 1440, fps: 30 }, null)).toEqual({
      width: 2560,
      height: 1440,
      fps: 30,
    });
  });

  it("liveSource: 원본 크기는 통계의 sourceWidth, 모니터 주사율은 모른다(fps는 설정 그대로, D-107)", () => {
    const base = { width: 1920, height: 1080, video: { sourceWidth: 2560, sourceHeight: 1440 } };
    expect(liveSource(base)).toEqual({ width: 2560, height: 1440, refreshRate: null });
    // 첫 프레임 전(sourceWidth 0)이면 출력 크기
    expect(liveSource({ ...base, video: { sourceWidth: 0, sourceHeight: 0 } })).toEqual({ width: 1920, height: 1080, refreshRate: null });
  });

  it("fpsHint", () => {
    expect(fpsHint(60, 30)).toBe("이 화면은 30Hz라 60fps를 골라도 30fps까지만 나옵니다.");
    expect(fpsHint(60, 60)).toBeNull();
    expect(fpsHint(30, 144)).toBeNull();
    expect(fpsHint(60, null)).toBeNull();
  });

  it("settingsFor", () => {
    expect(settingsFor({ width: 0, height: 0, fps: 0 }, 3000)).toEqual({
      width: 0,
      height: 0,
      fps: 0,
      bitrateKbps: 3000,
    });
  });
});

describe("LatestOnly(방송 중 자동 적용)", () => {
  /** 보낼 때마다 끝나지 않은 약속을 만들고, 시험이 직접 끝낸다. */
  function harness() {
    const sent: number[] = [];
    const waits: ((ok: boolean) => void)[] = [];
    const idle: boolean[] = [];
    const q = new LatestOnly<number>(
      (v) => {
        sent.push(v);
        return new Promise<boolean>((r) => waits.push(r));
      },
      (ok) => idle.push(ok),
    );
    const finish = async (ok = true) => {
      waits.shift()?.(ok);
      await new Promise((r) => setTimeout(r, 0));
    };
    return { q, sent, idle, finish };
  }

  it("보내는 중에 들어온 값은 마지막 것 하나만 이어서 보낸다", async () => {
    const h = harness();
    void h.q.push(1);
    void h.q.push(2);
    void h.q.push(3);
    void h.q.push(4);
    expect(h.sent).toEqual([1]);
    expect(h.q.sending).toBe(true);
    await h.finish();
    expect(h.sent).toEqual([1, 4]);
    await h.finish();
    expect(h.q.sending).toBe(false);
    expect(h.idle).toEqual([true]);
  });

  it("바로 전과 같은 값은 다시 보내지 않는다(mark로 알려 둔 값도)", async () => {
    const h = harness();
    h.q.mark(7);
    void h.q.push(7);
    expect(h.sent).toEqual([]);
    void h.q.push(8);
    await h.finish();
    void h.q.push(8);
    expect(h.sent).toEqual([8]);
  });

  it("실패하면 기다리던 값을 버리고, 같은 값을 다시 보낼 수 있다", async () => {
    const h = harness();
    void h.q.push(1);
    void h.q.push(2);
    await h.finish(false);
    expect(h.sent).toEqual([1]);
    expect(h.idle).toEqual([false]);
    void h.q.push(1);
    expect(h.sent).toEqual([1, 1]);
    await h.finish();
    expect(h.idle).toEqual([false, true]);
  });

  it("send가 던져도 실패로 본다", async () => {
    const idle: boolean[] = [];
    const q = new LatestOnly<number>(() => Promise.reject(new Error("x")), (ok) => idle.push(ok));
    await q.push(1);
    expect(idle).toEqual([false]);
    expect(q.sending).toBe(false);
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
    settings: { width: 1920, height: 1080, fps: 60, bitrateKbps: 7000 },
    video: {
      encoder: "h264_nvenc",
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
      targetKbps: 7000,
      maxKbps: 7000,
      estimateKbps: 21000,
      limited: false,
    },
  } as unknown as BroadcastStatus;

  it("videoRows", () => {
    const rows = Object.fromEntries(videoRows(status).map((r) => [r.label, r.value]));
    expect(rows["캡처"]).toBe("ELDEN RING™");
    expect(rows["출력"]).toBe("1920×1080");
    expect(rows["인코더"]).toBe("h264_nvenc");
    expect(rows["프레임"]).toBe("캡처 60 / 인코딩 60 fps · 버림 3");
    expect(rows["인코딩"]).toBe("평균 2.1 ms · 변환 0.4 ms");
    expect(rows["캡처→출력"]).toBe("12.3 ms");
  });
  it("다음 창을 기다리는 중이면 캡처 줄에 알린다", () => {
    const row = videoRows({ ...status, sourceWaiting: true }).find((r) => r.label === "캡처");
    expect(row).toEqual({ label: "캡처", value: "ELDEN RING™ · 다음 창을 기다리는 중", tone: "warn" });
  });
  it("인코더를 열기 전(이름 없음)이면", () => {
    const rows = videoRows({ ...status, video: { ...status.video, encoder: "" } });
    expect(rows.find((r) => r.label === "인코더")?.value).toBe("–");
  });
  it("sendRows", () => {
    expect(sendRows(status.publish).map((r) => r.label + "=" + r.value)).toEqual([
      "비디오=6.95 Mbps",
      "목표=7.00 Mbps",
      "추정 대역폭=21.00 Mbps",
      "오디오=128 kbps · RED 포함",
      "RTT=4 ms",
      "손실=1.2%",
      "키프레임 요청=1",
    ]);
    expect(sendRows({ ...status.publish, audioKbps: 0 }).find((r) => r.label === "오디오")?.value).toBe("0 kbps");
    expect(sendRows({ ...status.publish, rttMs: null, loss: null }).map((r) => r.value)).toContain("– ms");
    expect(sendRows({ ...status.publish, loss: null }).find((r) => r.label === "손실")?.value).toBe("0.0%");
    expect(sendRows({ ...status.publish, estimateKbps: null }).find((r) => r.label === "추정 대역폭")?.value).toBe("–");
  });
  it("네트워크 때문에 낮추면 목표 줄이 경고이고 화질 패널에 안내가 뜬다", () => {
    const limited = { ...status.publish, targetKbps: 2500, estimateKbps: 2950, limited: true };
    expect(sendRows(limited).find((r) => r.label === "목표")).toEqual({
      label: "목표",
      value: "2.50 Mbps · 네트워크에 맞춰 낮춤",
      tone: "warn",
    });
    expect(networkLimitHint(limited)).toBe(
      "네트워크가 붐벼서 지금은 2.50 Mbps로 보내고 있습니다(설정 7.00 Mbps). 회선이 나아지면 자동으로 올립니다.",
    );
    expect(sendRows(limited).find((r) => r.label === "추정 대역폭")?.value).toBe("2.95 Mbps");
    expect(networkLimitHint(status.publish)).toBeNull();
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
    expect(discordRow("app", { discordExcluded: false })).toBeNull();
    expect(discordRow("systemExceptDiscord", null)).toBeNull();
  });
});

describe("캡처 대상 미리 보기", () => {
  const shot = (id: string, image: string | null, minimized = false): SourceThumbnail => ({ id, image, minimized });

  it("thumbnailIds는 테스트 패턴을 뺀다", () => {
    expect(
      thumbnailIds([src({ id: "monitor:1" }), src({ id: "window:2", kind: "window" }), src({ id: "test", kind: "test" })]),
    ).toEqual(["monitor:1", "window:2"]);
  });

  it("새 그림과 최소화는 그대로 받는다", () => {
    const out = mergeThumbnails({}, [shot("a", "data:a"), shot("b", null, true)], ["a", "b"]);
    expect(out).toEqual({ a: shot("a", "data:a"), b: shot("b", null, true) });
  });

  it("이번에 못 받았으면 지난 그림을 둔다", () => {
    const prev = { a: shot("a", "data:old") };
    expect(mergeThumbnails(prev, [shot("a", null)], ["a"])).toEqual(prev);
    expect(mergeThumbnails(prev, [], ["a"])).toEqual(prev);
  });

  it("최소화되면 지난 그림 대신 최소화로", () => {
    const out = mergeThumbnails({ a: shot("a", "data:old") }, [shot("a", null, true)], ["a"]);
    expect(out.a).toEqual(shot("a", null, true));
  });

  it("목록에서 빠진 대상은 버린다", () => {
    expect(mergeThumbnails({ gone: shot("gone", "data:x") }, [shot("a", "data:a")], ["a"])).toEqual({
      a: shot("a", "data:a"),
    });
  });
});

describe("캡처 대상에 따른 소리 모드", () => {
  const app = src({ kind: "window", processName: "chrome.exe" });
  const discord = src({ kind: "window", processName: "Discord.exe", discord: true });
  const monitor = src({ kind: "monitor" });
  const test = src({ kind: "test", id: "test" });

  it("appAudioAllowed: Discord가 아닌 창만", () => {
    expect(appAudioAllowed(app)).toBe(true);
    expect(appAudioAllowed(discord)).toBe(false);
    expect(appAudioAllowed(monitor)).toBe(false);
    expect(appAudioAllowed(test)).toBe(false);
    expect(appAudioAllowed(null)).toBe(false);
  });

  it("창이면 그 앱의 소리만, 화면이면 시스템 소리에서 Discord만 뺀다", () => {
    expect(audioForSource(app)).toBe("app");
    expect(audioForSource(monitor)).toBe("systemExceptDiscord");
  });

  it("Discord 창은 앱 소리를 쓰지 않는다", () => {
    expect(audioForSource(discord)).toBe("systemExceptDiscord");
  });

  it("테스트 패턴은 테스트 톤, 고른 것이 없으면 시스템 − Discord", () => {
    expect(audioForSource(test)).toBe("testTone");
    expect(audioForSource(null)).toBe("systemExceptDiscord");
  });
});

describe("게임 창 따라가기 안내", () => {
  const lol = src({ kind: "window", processName: "LeagueClientUx.exe", follow: "lol" });
  it("롤 창이 아니면 없음", () => {
    expect(followNote(src({ kind: "window" }), true)).toBeNull();
    expect(followNote(null, true)).toBeNull();
  });
  it("앱 소리면 화면과 소리가 같이 넘어간다", () => {
    expect(followNote(lol, true)).toBe(
      "롤 창: 게임이 시작되면 게임 화면과 소리로 넘어가고, 게임이 끝나면 클라이언트로 돌아옵니다.",
    );
  });
  it("앱 소리가 아니면 화면만", () => {
    expect(followNote(lol, false)).toBe(
      "롤 창: 게임이 시작되면 게임 화면으로 넘어가고, 게임이 끝나면 클라이언트로 돌아옵니다.",
    );
  });
});

describe("화면 전환(D-086)", () => {
  const window = src({ id: "window:1", kind: "window", name: "YouTube - Chrome", processName: "chrome.exe" });
  const monitor = src({ id: "monitor:1", kind: "monitor", primary: true });
  const discord = src({ id: "window:2", kind: "window", name: "Discord", processName: "Discord.exe", discord: true });
  const lol = src({ id: "window:3", kind: "window", name: "League of Legends", processName: "LeagueClientUx.exe", follow: "lol" });

  it("테스트 패턴은 고를 수 없고, 테스트 패턴 방송은 바꿀 수 없다", () => {
    const test = src({ id: "test", kind: "test" });
    expect(switchableSources([monitor, test, window]).map((s) => s.id)).toEqual(["monitor:1", "window:1"]);
    expect(canSwitchSource({ sourceId: "test" })).toBe(false);
    expect(canSwitchSource({ sourceId: "monitor:1" })).toBe(true);
  });

  it("창이면 앱 소리, 앱 소리였는데 화면·Discord 창이면 시스템 − Discord로 바뀐다고 알린다", () => {
    expect(switchNotes(window, "systemExceptDiscord")).toEqual(["소리는 이 앱의 소리만 보냅니다."]);
    expect(switchNotes(window, "app")).toEqual(["소리는 이 앱의 소리만 보냅니다."]);
    expect(switchNotes(monitor, "app")).toEqual(["소리는 시스템 소리(Discord 제외)로 바뀝니다."]);
    expect(switchNotes(discord, "app")).toEqual(["소리는 시스템 소리(Discord 제외)로 바뀝니다."]);
    expect(switchNotes(monitor, "systemExceptDiscord")).toEqual([]);
    expect(switchNotes(discord, "systemExceptDiscord")).toEqual([]);
  });

  it("소리가 없는 방송·테스트 톤은 소리 안내가 없고, 고른 것이 없으면 아무것도 없다", () => {
    expect(switchNotes(window, "testTone")).toEqual([]);
    expect(switchNotes(window, null)).toEqual([]);
    expect(switchNotes(null, "app")).toEqual([]);
  });

  it("롤 창이면 따라가기 안내를 더한다(소리가 있으면 소리도 따라간다)", () => {
    expect(switchNotes(lol, "systemExceptDiscord")).toEqual([
      "소리는 이 앱의 소리만 보냅니다.",
      "롤 창: 게임이 시작되면 게임 화면과 소리로 넘어가고, 게임이 끝나면 클라이언트로 돌아옵니다.",
    ]);
    expect(switchNotes(lol, null)).toEqual([
      "롤 창: 게임이 시작되면 게임 화면으로 넘어가고, 게임이 끝나면 클라이언트로 돌아옵니다.",
    ]);
  });
});

describe("normalizeAudioBoost", () => {
  it("100~300% 안의 10% 단위로 맞춘다", () => {
    expect(normalizeAudioBoost(250)).toBe(250);
    expect(normalizeAudioBoost(254)).toBe(250);
    expect(normalizeAudioBoost(256)).toBe(260);
    expect(normalizeAudioBoost(50)).toBe(100);
    expect(normalizeAudioBoost(800)).toBe(300);
    expect(normalizeAudioBoost(5000)).toBe(300);
  });

  it("숫자가 아니면 100(그대로)", () => {
    expect(normalizeAudioBoost(null)).toBe(100);
    expect(normalizeAudioBoost(undefined)).toBe(100);
    expect(normalizeAudioBoost(Number.NaN)).toBe(100);
  });
});
