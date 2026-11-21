import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BroadcastThumbnail } from "$lib/types";

const mocks = vi.hoisted(() => ({
  broadcastThumbnail: vi.fn(),
  handlers: {} as Record<string, (e: { payload: unknown }) => void>,
}));

vi.mock("$lib/api", () => ({
  api: { broadcastThumbnail: mocks.broadcastThumbnail },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: (e: { payload: unknown }) => void) => {
    mocks.handlers[name] = cb;
    return () => {};
  },
}));

import {
  onThumbnailEvent,
  resetThumbnails,
  thumbKey,
  thumbnailOf,
  thumbs,
  trackThumbnail,
  watchTarget,
} from "./thumbnails.svelte";

const pic = (id: number): BroadcastThumbnail => ({ id, image: `data:image/jpeg;base64,${id}` });
const target = { server: "srv", broadcaster: "b1" };
/** 기다리던 응답이 화면 상태에 들어갈 때까지 */
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  resetThumbnails();
  mocks.broadcastThumbnail.mockReset();
});

describe("watchTarget", () => {
  it("시청 주소에서 서버 key와 방송자 ID를 읽는다", () => {
    expect(watchTarget("#/watch/srv/b1")).toEqual({ server: "srv", broadcaster: "b1" });
    expect(watchTarget("#/watch/local-AbC_-9/Zx-1?popout=1")).toEqual({
      server: "local-AbC_-9",
      broadcaster: "Zx-1",
    });
  });

  it("시청 주소가 아니면 null(내 방송 타일, 링크 없음)", () => {
    expect(watchTarget("#/broadcast")).toBeNull();
    expect(watchTarget(undefined)).toBeNull();
    expect(watchTarget("#/watch/srv")).toBeNull();
    expect(watchTarget("#/server/srv")).toBeNull();
  });
});

describe("trackThumbnail", () => {
  it("처음 보이면 묻고, 받은 그림을 서버 key + 방송자 ID로 찾는다", async () => {
    mocks.broadcastThumbnail.mockResolvedValue(pic(3));
    trackThumbnail(target);
    await flush();
    expect(mocks.broadcastThumbnail).toHaveBeenCalledWith("srv", "b1");
    expect(thumbnailOf(target)).toBe("data:image/jpeg;base64,3");
    expect(thumbs.images[thumbKey("srv", "b1")]).toBe("data:image/jpeg;base64,3");
    expect(thumbnailOf({ server: "other", broadcaster: "b1" })).toBeNull();
    expect(thumbnailOf(null)).toBeNull();
  });

  it("같은 그림을 여러 타일이 보여도 한 번만 묻는다", async () => {
    mocks.broadcastThumbnail.mockResolvedValue(pic(3));
    trackThumbnail(target);
    trackThumbnail(target);
    await flush();
    expect(mocks.broadcastThumbnail).toHaveBeenCalledTimes(1);
  });

  it("그림이 없으면(null) 자리 표시, 실패하면 지난 그림을 둔다", async () => {
    mocks.broadcastThumbnail.mockResolvedValueOnce(null);
    const stop = trackThumbnail(target);
    await flush();
    expect(thumbnailOf(target)).toBeNull();
    stop();

    mocks.broadcastThumbnail.mockResolvedValueOnce(pic(4));
    const stop2 = trackThumbnail(target);
    await flush();
    expect(thumbnailOf(target)).not.toBeNull();
    stop2();
    mocks.broadcastThumbnail.mockRejectedValueOnce(new Error("연결이 끊어졌습니다"));
    trackThumbnail(target);
    await flush();
    expect(thumbnailOf(target)).toBe("data:image/jpeg;base64,4");
  });
});

describe("broadcast-thumbnail 이벤트", () => {
  it("보이는 타일은 번호가 바뀌면 다시 묻고, 같은 번호면 묻지 않는다", async () => {
    mocks.broadcastThumbnail.mockResolvedValueOnce(pic(1)).mockResolvedValueOnce(pic(2));
    trackThumbnail(target);
    await flush();
    onThumbnailEvent({ key: "srv", broadcasterId: "b1", id: 1 });
    await flush();
    expect(mocks.broadcastThumbnail).toHaveBeenCalledTimes(1);
    // 백엔드가 Tauri 이벤트로 보내는 것도 같은 처리다.
    mocks.handlers["broadcast-thumbnail"]({ payload: { key: "srv", broadcasterId: "b1", id: 2 } });
    await flush();
    expect(mocks.broadcastThumbnail).toHaveBeenCalledTimes(2);
    expect(thumbnailOf(target)).toBe("data:image/jpeg;base64,2");
  });

  it("보이지 않는 타일은 묻지 않고, 다음에 보일 때 묻는다", async () => {
    mocks.broadcastThumbnail.mockResolvedValue(pic(7));
    onThumbnailEvent({ key: "srv", broadcasterId: "b1", id: 7 });
    await flush();
    expect(mocks.broadcastThumbnail).not.toHaveBeenCalled();
    trackThumbnail(target);
    await flush();
    expect(thumbnailOf(target)).toBe("data:image/jpeg;base64,7");
  });

  it("묻는 동안 번호가 또 바뀌면 끝난 뒤 한 번 더 묻는다", async () => {
    let first!: (v: BroadcastThumbnail) => void;
    mocks.broadcastThumbnail
      .mockReturnValueOnce(new Promise<BroadcastThumbnail>((r) => (first = r)))
      .mockResolvedValueOnce(pic(9));
    trackThumbnail(target);
    onThumbnailEvent({ key: "srv", broadcasterId: "b1", id: 9 });
    onThumbnailEvent({ key: "srv", broadcasterId: "b1", id: 9 });
    first(pic(8));
    await flush();
    await flush();
    expect(mocks.broadcastThumbnail).toHaveBeenCalledTimes(2);
    expect(thumbnailOf(target)).toBe("data:image/jpeg;base64,9");
  });

  it("번호가 없어지면(방송 끝) 그림을 지운다", async () => {
    mocks.broadcastThumbnail.mockResolvedValue(pic(1));
    trackThumbnail(target);
    await flush();
    onThumbnailEvent({ key: "srv", broadcasterId: "b1", id: null });
    expect(thumbnailOf(target)).toBeNull();
  });

  it("서버 연결이 끊기면 그 서버의 그림만 지운다", async () => {
    mocks.broadcastThumbnail.mockImplementation(async (key: string, id: string) => ({
      id: 1,
      image: `${key}:${id}`,
    }));
    trackThumbnail(target);
    trackThumbnail({ server: "srv", broadcaster: "b2" });
    trackThumbnail({ server: "srv2", broadcaster: "b1" });
    await flush();
    mocks.handlers["server-closed"]({ payload: { key: "srv", reason: "x" } });
    expect(thumbnailOf(target)).toBeNull();
    expect(thumbnailOf({ server: "srv", broadcaster: "b2" })).toBeNull();
    expect(thumbnailOf({ server: "srv2", broadcaster: "b1" })).toBe("srv2:b1");
  });
});
