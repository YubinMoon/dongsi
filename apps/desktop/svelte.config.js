// Tauri에는 SSR을 돌릴 Node.js 서버가 없으므로 adapter-static으로 SPA를 만든다.
// 경로는 해시(#/server/...)로 관리한다. 해시 라우터는 index.html 하나만 만들고 SSR을 하지 않으므로
// fallback 페이지나 `ssr = false` 같은 페이지 옵션이 필요 없다(설정하면 빌드 오류).
// https://svelte.dev/docs/kit/single-page-apps
// https://v2.tauri.app/start/frontend/sveltekit/
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter(),
    router: {
      type: "hash",
    },
  },
};

export default config;
