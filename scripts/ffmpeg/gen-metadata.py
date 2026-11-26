#!/usr/bin/env python3
"""licenses/components.json과 BUILDINFO.md를 만든다(inside-build.sh가 호출).

build.sh가 ffcheck를 돌린 뒤 "실행 확인" 절을 BUILDINFO.md 뒤에 덧붙인다.
"""
import argparse
import json
import re
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument("--facts", required=True)
ap.add_argument("--work", required=True)
ap.add_argument("--stage", required=True)
ap.add_argument("--ffmpeg-version", required=True)
ap.add_argument("--build-id", required=True)
ap.add_argument("--build-date", required=True)
a = ap.parse_args()

facts = Path(a.facts)
stage = Path(a.stage)
work = Path(a.work)


def read(name):
    return (facts / name).read_text(encoding="utf-8").strip()


downloads = {}
for line in read("downloads.txt").splitlines():
    f, url, sha = line.split("|")
    downloads[f] = (url, sha)

license_files = {}
for line in (work / "license-files.txt").read_text(encoding="utf-8").splitlines():
    cid, files = line.split("|", 1)
    license_files[cid] = files.split()

components = []
for line in read("components.txt").splitlines():
    cid, name, version, spdx, home, srcfile, linkage, _lic, notes = line.split("|")
    if srcfile == "-":
        url, sha = "빌더 이미지(우분투 24.04 패키지)에서 옴", ""
    else:
        url, sha = downloads[srcfile]
    components.append(
        {
            "id": cid,
            "name": name,
            "version": version,
            "license": spdx,
            "homepage": home,
            "source": url,
            "sha256": sha,
            "linkage": linkage,
            "licenseFiles": license_files[cid],
            "notes": notes,
        }
    )

ff = next(c for c in components if c["id"] == "ffmpeg")
configure = read("configure.txt")
bundle_name = read("bundle-name.txt")
bundle_sha = read("bundle-sha256.txt")

doc = {
    "ffmpeg": {
        "version": a.ffmpeg_version,
        "configure": configure,
        "license": ff["license"],
    },
    "sourceBundle": {"file": bundle_name, "sha256": bundle_sha},
    "components": components,
}
(stage / "licenses").mkdir(parents=True, exist_ok=True)
(stage / "licenses" / "components.json").write_text(
    json.dumps(doc, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
)

# ---- BUILDINFO.md ----
sizes = []
for line in read("sizes.txt").splitlines():
    n, s = line.rsplit(" ", 1)
    sizes.append((Path(n).name, int(s)))

imports = read("imports.txt")
tools = read("tools.txt").splitlines()

md = []
md.append("# BUILDINFO — dongsi 최소 LGPL ffmpeg")
md.append("")
md.append("이 파일은 scripts/ffmpeg/build.sh가 만든다. 이 폴더의 DLL이 어떤 소스와 도구로 만들어졌는지 적는다.")
md.append("")
md.append(f"- FFmpeg 버전: {a.ffmpeg_version} (릴리스 tarball, 서명 파일은 소스 묶음에 있음)")
md.append(f"- 빌드 id: {a.build_id}")
md.append(f"- 빌드 날짜(UTC): {a.build_date}")
md.append("- 대상: Windows x64 (x86_64-w64-mingw32 교차 빌드, 스레드 모델 posix, FFmpeg 스레드는 w32threads)")
md.append("- 라이선스: LGPL 2.1 이상 (--enable-gpl, --enable-version3, --enable-nonfree 없음)")
md.append("")
md.append("## configure 줄 (config.h의 FFMPEG_CONFIGURATION)")
md.append("")
md.append("```")
md.append(configure)
md.append("```")
md.append("")
md.append("## 구성 요소")
md.append("")
md.append("| 구성 요소 | 버전 | 라이선스 | 링크 | 소스 URL | 소스 SHA-256 |")
md.append("|---|---|---|---|---|---|")
for c in components:
    md.append(
        f"| {c['name']} | {c['version']} | {c['license']} | {c['linkage']} | {c['source']} | {c['sha256'] or '-'} |"
    )
md.append("")
md.append("라이선스 파일은 licenses/<id>/에, 기계가 읽는 목록은 licenses/components.json에 있다.")
md.append("")
md.append("### 구성 요소별 비고")
md.append("")
for c in components:
    if c["notes"]:
        md.append(f"- {c['name']}: {c['notes']}")
md.append("")
md.append("## 도구")
md.append("")
for t in tools:
    md.append(f"- {t}")
md.append("")
md.append("## DLL (strip 뒤)")
md.append("")
md.append("| 파일 | 크기(바이트) |")
md.append("|---|---|")
for n, s in sizes:
    md.append(f"| {n} | {s:,} |")
md.append("")
md.append("### DLL이 가져오는 DLL (objdump -p 의 DLL Name)")
md.append("")
md.append("윈도우 시스템 DLL과 ffmpeg DLL만 있어야 한다(libgcc, libstdc++, libwinpthread, libssp를 가져오지 않음).")
md.append("")
md.append("msvcrt.dll은 윈도우에 항상 있는 시스템 DLL이다. 우분투 mingw-w64가 기본으로 쓰는 C 런타임이라 UCRT(api-ms-win-crt-*)가 아니라 이것을 가져온다.")
md.append("")
md.append("```")
md.append(imports)
md.append("```")
md.append("")
md.append("## 인코더·디코더 (configure에서 켠 것)")
md.append("")
md.append("- 인코더: " + ", ".join(read("encoders.txt").splitlines()))
md.append("- 디코더(record 예제의 --decode 검사용): " + ", ".join(read("decoders.txt").splitlines()))
md.append("")
md.append("## 소스 묶음")
md.append("")
md.append(f"- 파일: source/{bundle_name}")
md.append(f"- SHA-256: {bundle_sha}")
md.append("- 내용: 위 구성 요소의 소스 원본 파일 그대로 + scripts/ffmpeg 사본(빌드 스크립트·Dockerfile) + SHA256SUMS + 다시 빌드하는 방법(REBUILD.md)")
md.append("")
md.append("## 빌더")
md.append("")
md.append("scripts/ffmpeg/Dockerfile의 이미지(ubuntu:24.04를 다이제스트로 고정). 도구 버전은 위 도구 절, 이미지 ID는 도구 절의 마지막 줄.")
md.append("")
(stage / "BUILDINFO.md").write_text("\n".join(md) + "\n", encoding="utf-8")
print("components.json, BUILDINFO.md 작성됨")
