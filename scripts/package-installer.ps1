# 설치 파일(NSIS) 만들기. Windows PowerShell 5.1 이상.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package-installer.ps1 -FfmpegDir <FFmpeg LGPL 폴더>
#
# 결과: dist\dongsi-<버전>-windows-x64-setup.exe (와 .sha256)
#   사용자별 설치(관리자 권한 없음, 기본 위치 %LOCALAPPDATA%\dongsi). 설치 폴더에는 dongsi.exe, FFmpeg DLL,
#   LICENSE, THIRD_PARTY_NOTICES.md, licenses\, README.txt가 들어간다(D-063, docs/licensing.md).
#   시작 메뉴 바로 가기 이름은 "동시"다(윈도우 검색용, tauri.conf.json의 installerHooks =
#   apps\desktop\src-tauri\windows\installer-hooks.nsh, D-064).
#
# 인자
#   -FfmpegDir   FFmpeg LGPL 빌드 폴더(scripts/ffmpeg/build.sh의 결과). 기본은 환경 변수 FFMPEG_DIR.
#                licenses\components.json이 없거나 configure에 --enable-gpl·--enable-nonfree가 있으면 멈춘다.
#   -OutDir      결과를 둘 폴더. 기본은 저장소 루트의 dist\
#
# tauri.conf.json은 bundle.active = false다. 그냥 `tauri build`로는 FFmpeg DLL이 빠진(실행되지 않는) 설치 파일이
# 만들어지므로 막아 두었다. 이 스크립트가 FFmpeg DLL·라이선스 파일(bundle.resources)과 active = true를
# --config로 덧붙인다. Windows에서 Tauri의 resource 폴더는 exe가 있는 폴더라서 DLL이 dongsi.exe 옆에 놓인다.
# 처음 빌드할 때 Tauri CLI가 NSIS를 내려받는다(인터넷 필요). 빌드 환경은 AGENTS.md(LIBCLANG_PATH 등).
param(
  [string]$FfmpegDir = $env:FFMPEG_DIR,
  [string]$OutDir
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'lib\release.ps1')
if (-not $OutDir) { $OutDir = Join-Path $repo 'dist' }

# ---------- 1. FFmpeg 폴더 확인(GPL·nonfree면 멈춘다) ----------
$ff = Get-LgplFfmpeg $FfmpegDir $repo
$ver = Get-AppVersion $repo
$targetDir = Get-TargetDir $repo
New-Item -ItemType Directory -Force -Path $targetDir | Out-Null
$targetDir = (Resolve-Path -LiteralPath $targetDir).Path

# ---------- 2. 설치 폴더에 넣을 파일 모으기 ----------
$stage = Join-Path $targetDir 'installer-resources'
Remove-OutputDir $stage $targetDir
New-Item -ItemType Directory -Force -Path $stage | Out-Null
foreach ($d in $ff.Dlls) { Copy-Item -LiteralPath $d.FullName -Destination $stage }
Copy-LicenseFiles $repo $ff $stage
Write-ReleaseReadme (Join-Path $stage 'README.txt') $ver $ff 'installer'

# bundle.resources: 원본(절대 경로) -> 설치 폴더 안의 상대 경로. 파일마다 하나씩 적는다.
$resources = [ordered]@{}
Get-ChildItem -LiteralPath $stage -Recurse -File | Sort-Object FullName | ForEach-Object {
  $rel = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
  $resources[$_.FullName.Replace('\', '/')] = $rel
}
$overlay = [ordered]@{ bundle = [ordered]@{ active = $true; targets = @('nsis'); resources = $resources } }
$overlayPath = Join-Path $targetDir 'installer-tauri.conf.json'
[System.IO.File]::WriteAllText($overlayPath, ($overlay | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding($false)))

# ---------- 3. 빌드 ----------
$env:FFMPEG_DIR = $ff.Dir
$env:PATH = "$($ff.Bin);$env:PATH"
$tauriArgs = @('tauri', 'build', '--bundles', 'nsis', '--config', $overlayPath)
Write-Host ("[빌드] npx " + ($tauriArgs -join ' ')) -ForegroundColor Cyan
Push-Location (Join-Path $repo 'apps\desktop')
try {
  & npx @tauriArgs
  if ($LASTEXITCODE -ne 0) { Fail "빌드에 실패했습니다(종료 코드 $LASTEXITCODE)." }
} finally { Pop-Location }

$exe = Get-AppExe $targetDir 'release' $repo
if (-not (Test-Path -LiteralPath $exe)) { Fail "exe가 없습니다: $exe" }
Test-DllImports $exe $ff.Dlls

$nsisDir = Join-Path $targetDir 'release\bundle\nsis'
$built = @(Get-ChildItem -LiteralPath $nsisDir -Filter "*_$($ver)_x64-setup.exe" -File | Sort-Object LastWriteTime -Descending)
if ($built.Count -eq 0) { Fail "설치 파일이 없습니다: $nsisDir" }

# ---------- 4. 결과 ----------
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$setup = Join-Path $OutDir "dongsi-$ver-windows-x64-setup.exe"
Copy-Item -LiteralPath $built[0].FullName -Destination $setup -Force
# Get-FileHash는 PSModulePath가 다른 셸(pwsh, Git Bash)에서 띄우면 없을 수 있어 .NET으로 계산한다.
$sha = [System.Security.Cryptography.SHA256]::Create()
$fs = [System.IO.File]::OpenRead($setup)
try { $hash = -join ($sha.ComputeHash($fs) | ForEach-Object { $_.ToString('x2') }) } finally { $fs.Dispose(); $sha.Dispose() }
[System.IO.File]::WriteAllText("$setup.sha256", "$hash  $(Split-Path -Leaf $setup)`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Host ''
Write-Host "만들었습니다: $setup" -ForegroundColor Green
Write-Host ('크기: {0:N0} 바이트, SHA-256: {1}' -f (Get-Item -LiteralPath $setup).Length, $hash)
Write-Host '설치 폴더에 들어가는 파일(dongsi.exe 말고):'
$resources.Values | ForEach-Object { '  ' + $_ }
