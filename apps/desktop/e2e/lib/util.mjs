// 공용 작은 도구: 기다리기, PNG 크기, 포트 검사, LAN 주소.
import dgram from "node:dgram";
import net from "node:net";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * fn이 참 같은 값을 돌려줄 때까지 interval마다 부른다. 시간이 지나면 desc와 함께 던진다.
 * fn이 던진 예외는 무시하고 다시 시도한다(마지막 예외는 오류 문장에 붙는다).
 */
export async function waitUntil(fn, { timeout = 10000, interval = 250, desc = "조건" } = {}) {
  const end = Date.now() + timeout;
  let lastErr = null;
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
      lastErr = null;
    } catch (e) {
      lastErr = e;
    }
    if (Date.now() >= end) {
      throw new Error(`시간 초과(${timeout}ms): ${desc}${lastErr ? " — " + String(lastErr.message ?? lastErr).split("\n")[0] : ""}`);
    }
    await sleep(interval);
  }
}

/** PNG 머리에서 가로·세로를 읽는다. */
export function pngSize(buf) {
  if (buf.length < 24 || buf.toString("ascii", 1, 4) !== "PNG") return null;
  return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
}

/** 127.0.0.1:port에 TCP 연결을 시도한다. "open" | "refused" | "timeout" | "error:코드" */
export function tcpProbe(port, host = "127.0.0.1", timeout = 2000) {
  return new Promise((resolve) => {
    const s = net.connect({ port, host });
    const done = (v) => {
      s.destroy();
      resolve(v);
    };
    s.setTimeout(timeout, () => done("timeout"));
    s.once("connect", () => done("open"));
    s.once("error", (e) => done(e.code === "ECONNREFUSED" ? "refused" : "error:" + e.code));
  });
}

/** 이 PC의 대표 LAN IPv4(기본 경로가 쓰는 주소). 패킷은 보내지 않는다. */
export function primaryLanIp() {
  return new Promise((resolve) => {
    const s = dgram.createSocket("udp4");
    s.once("error", () => {
      s.close();
      resolve(null);
    });
    s.connect(53, "8.8.8.8", () => {
      const a = s.address().address;
      s.close();
      resolve(a);
    });
  });
}

/** 연결은 받고 아무 말도 하지 않는 TCP 서버("연결하는 중" 재현용). */
export function startHangServer(port) {
  const sockets = new Set();
  const server = net.createServer((sock) => {
    sockets.add(sock);
    sock.on("error", () => {});
    sock.on("close", () => sockets.delete(sock));
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () =>
      resolve({
        port,
        close: () =>
          new Promise((r) => {
            for (const s of sockets) s.destroy();
            server.close(() => r());
          }),
      }),
    );
  });
}

/** TCP와 UDP 포트를 모두 잡아 둔다(내장 서버의 포트 대체 재현용). 0.0.0.0에 묶는다. */
export async function occupyPort(port) {
  const tcp = net.createServer((s) => s.destroy());
  await new Promise((resolve, reject) => {
    tcp.once("error", reject);
    tcp.listen(port, "0.0.0.0", resolve);
  });
  const udp = dgram.createSocket("udp4");
  try {
    await new Promise((resolve, reject) => {
      udp.once("error", reject);
      udp.bind(port, "0.0.0.0", resolve);
    });
  } catch (e) {
    await new Promise((r) => tcp.close(() => r()));
    throw e;
  }
  return {
    port,
    release: async () => {
      await new Promise((r) => tcp.close(() => r()));
      await new Promise((r) => udp.close(() => r()));
    },
  };
}
