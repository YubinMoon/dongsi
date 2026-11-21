import { describe, expect, it } from "vitest";
import { decodePreview, previewRequestSize } from "./livePreview";

function frame(seq: number, width: number, height: number, fill = 7): ArrayBuffer {
  const buf = new ArrayBuffer(12 + width * height * 4);
  const view = new DataView(buf);
  view.setUint32(0, seq, true);
  view.setUint32(4, width, true);
  view.setUint32(8, height, true);
  new Uint8Array(buf, 12).fill(fill);
  return buf;
}

describe("decodePreview", () => {
  it("머리(번호·너비·높이) 뒤의 RGBA를 읽는다", () => {
    const img = decodePreview(frame(5, 2, 3));
    expect(img).not.toBeNull();
    expect([img!.seq, img!.width, img!.height]).toEqual([5, 2, 3]);
    expect(img!.rgba.length).toBe(2 * 3 * 4);
    expect(img!.rgba[0]).toBe(7);
  });
  it("바이트 배열·Uint8Array로 와도 읽는다", () => {
    const bytes = new Uint8Array(frame(1, 1, 1, 9));
    expect(decodePreview(Array.from(bytes))?.rgba[3]).toBe(9);
    expect(decodePreview(bytes)?.width).toBe(1);
  });
  it("비었거나 모양이 틀리면 null", () => {
    expect(decodePreview(new ArrayBuffer(0))).toBeNull();
    expect(decodePreview(null)).toBeNull();
    expect(decodePreview(undefined)).toBeNull();
    expect(decodePreview("x")).toBeNull();
    expect(decodePreview(frame(1, 0, 3))).toBeNull();
    expect(decodePreview(frame(1, 2, 2).slice(0, 20))).toBeNull();
  });
});

describe("previewRequestSize", () => {
  it("CSS 크기 × 화면 배율(반올림)", () => {
    expect(previewRequestSize(400, 225, 1.25)).toEqual({ width: 500, height: 281 });
    expect(previewRequestSize(400, 225, 1)).toEqual({ width: 400, height: 225 });
  });
  it("배율이 이상하면 1, 너무 작거나 숫자가 아니면 16", () => {
    expect(previewRequestSize(400, 225, 0)).toEqual({ width: 400, height: 225 });
    expect(previewRequestSize(400, 225, Number.NaN)).toEqual({ width: 400, height: 225 });
    expect(previewRequestSize(0, Number.NaN, 2)).toEqual({ width: 16, height: 16 });
  });
});
