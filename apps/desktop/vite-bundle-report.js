// @ts-nocheck 빌드 도구 파일(@types/node 없이 쓰는 순수 JS)이라 타입 검사를 건너뛴다.
// 프런트엔드 번들에 실제로 들어간 npm 패키지 목록을 파일로 남기는 Vite 플러그인.
// 제3자 고지 생성기(scripts/licenses/generate.mjs)가 `DONGSI_BUNDLE_REPORT=<파일>`을 주고 빌드해서 쓴다.
// 환경 변수가 없으면 아무것도 하지 않는다(보통 빌드·`tauri dev`에는 영향이 없다).
//
// 고르는 방법: 클라이언트 빌드의 결과 청크마다 `modules`(이 청크에 들어간 모듈)를 보고,
// 출력에 실제로 그려진(renderedLength > 0, 트리 셰이킹으로 사라지지 않은) 모듈 중
// node_modules 아래 것의 패키지(이름·버전·폴더)를 모은다. 빌드 도구(vite, typescript 등)는
// 번들에 모듈이 들어가지 않으므로 저절로 빠진다.
import fs from "node:fs";
import path from "node:path";

/** 모듈 파일에서 위로 올라가며 이름이 있는 가장 가까운 package.json을 찾는다(node_modules 아래만). */
function packageOf(id) {
  const file = id.split("?")[0].replace(/\\/g, "/");
  if (!file.includes("/node_modules/")) return null;
  let dir = path.dirname(file);
  while (dir.includes("/node_modules/")) {
    const manifest = path.join(dir, "package.json");
    if (fs.existsSync(manifest)) {
      try {
        const pkg = JSON.parse(fs.readFileSync(manifest, "utf8"));
        if (pkg.name && pkg.version) return { name: pkg.name, version: pkg.version, dir };
      } catch {
        // 이름 없는 package.json(예: {"type":"module"})은 건너뛴다.
      }
    }
    dir = path.dirname(dir);
  }
  return null;
}

export function bundleReport(reportFile) {
  return {
    name: "dongsi-bundle-report",
    apply: "build",
    generateBundle(_options, bundle) {
      // SvelteKit은 서버(prerender용)와 클라이언트를 따로 빌드한다. 배포되는 것은 클라이언트뿐이다.
      if (!reportFile || this.environment?.name !== "client") return;
      const found = new Map();
      for (const item of Object.values(bundle)) {
        if (item.type !== "chunk") continue;
        for (const [id, mod] of Object.entries(item.modules ?? {})) {
          if (!mod.renderedLength) continue;
          const pkg = packageOf(id);
          if (pkg) found.set(pkg.name + "@" + pkg.version, pkg);
        }
      }
      const list = [...found.values()].sort((a, b) =>
        a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : 1,
      );
      fs.writeFileSync(reportFile, JSON.stringify(list, null, 2));
    },
  };
}
