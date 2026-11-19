import { defineConfig } from "vitest/config";
import { sveltekit } from "@sveltejs/kit/vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
import { bundleReport } from "./vite-bundle-report.js";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  // bundleReport: DONGSI_BUNDLE_REPORT가 있을 때만 번들에 든 npm 패키지 목록을 남긴다(제3자 고지 생성용).
  plugins: [sveltekit(), bundleReport(process.env.DONGSI_BUNDLE_REPORT)],

  // 순수 로직 단위 시험(vitest). 화면 부품은 시험하지 않는다.
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    // 설정 > 제3자 고지 화면이 저장소 루트의 THIRD_PARTY_NOTICES.md를 ?raw로 읽는다(앱 폴더 밖).
    fs: { allow: ["../.."] },
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
