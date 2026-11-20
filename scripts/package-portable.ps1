# 포터블 폴더 만들기(설치 파일 아님). Windows PowerShell 5.1 이상.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package-portable.ps1 -FfmpegDir <FFmpeg LGPL 폴더>
#
# 결과: dist\dongsi-<버전>-windows-x64-portable[-debug]\ 폴더와 같은 이름의 .zip
#   dongsi.exe, FFmpeg DLL, LICENSE, THIRD_PARTY_NOTICES.md, licenses\, README.txt
#
# 인자
#   -FfmpegDir   FFmpeg LGPL 빌드 폴더(scripts/ffmpeg/build.sh의 결과). 기본은 환경 변수 FFMPEG_DIR.
#                licenses\components.json이 없거나 configure에 --enable-gpl·--enable-nonfree가 있으면 멈춘다.
#   -Debug       디버그 빌드(npx tauri build --no-bundle --debug)를 포장한다. 테스트 패턴 방송 확인용이고, 폴더 이름에 -debug가 붙는다.
#   -SkipBuild   빌드하지 않고 이미 만든 exe를 쓴다(CARGO_TARGET_DIR을 따른다).
#   -OutDir      결과를 둘 폴더. 기본은 저장소 루트의 dist\
#
# 빌드에는 AGENTS.md의 빌드 환경(LIBCLANG_PATH 등)이 필요하다. FFMPEG_DIR과 PATH는 이 스크립트가 FfmpegDir로 맞춘다.
# 설치 파일은 scripts/package-installer.ps1. 두 스크립트가 같이 쓰는 함수는 scripts/lib/release.ps1.
param(
  [string]$FfmpegDir = $env:FFMPEG_DIR,
  [switch]$Debug,
  [switch]$SkipBuild,
  [string]$OutDir
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'lib\release.ps1')
if (-not $OutDir) { $OutDir = Join-Path $repo 'dist' }

# ---------- 1. FFmpeg 폴더 확인(GPL·nonfree면 멈춘다) ----------
$ff = Get-LgplFfmpeg $FfmpegDir $repo
$FfmpegDir = $ff.Dir
$ffBin = $ff.Bin
$dlls = $ff.Dlls

# ---------- 2. 빌드 ----------
$config = if ($Debug) { 'debug' } else { 'release' }
$suffix = if ($Debug) { '-debug' } else { '' }
$targetDir = Get-TargetDir $repo

if (-not $SkipBuild) {
  $env:FFMPEG_DIR = $FfmpegDir
  $env:PATH = "$ffBin;$env:PATH"
  $tauriArgs = @('tauri', 'build', '--no-bundle')
  if ($Debug) { $tauriArgs += '--debug' }
  Write-Host ("[빌드] npx " + ($tauriArgs -join ' ')) -ForegroundColor Cyan
  Push-Location (Join-Path $repo 'apps\desktop')
  try {
    & npx @tauriArgs
    if ($LASTEXITCODE -ne 0) { Fail "빌드에 실패했습니다(종료 코드 $LASTEXITCODE)." }
  } finally { Pop-Location }
}
$exe = Get-AppExe $targetDir $config $repo
if (-not (Test-Path -LiteralPath $exe)) { Fail "exe가 없습니다: $exe (CARGO_TARGET_DIR을 확인하세요. -SkipBuild 없이 다시 실행해도 됩니다)" }

# ---------- 3. exe가 요구하는 DLL 점검(FFmpeg DLL과 시스템 DLL 말고는 없어야 한다) ----------
Test-DllImports $exe $dlls

# ---------- 4. 폴더 만들기 ----------
$ver = Get-AppVersion $repo
$name = "dongsi-$ver-windows-x64-portable$suffix"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$dest = Join-Path $OutDir $name
$zip = "$dest.zip"
Remove-OutputDir $dest $OutDir
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
New-Item -ItemType Directory -Force -Path $dest | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $dest 'dongsi.exe')
foreach ($d in $dlls) { Copy-Item -LiteralPath $d.FullName -Destination $dest }
Copy-LicenseFiles $repo $ff $dest
Write-ReleaseReadme (Join-Path $dest 'README.txt') $ver $ff 'portable' ([bool]$Debug)

# ---------- 5. zip ----------
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($dest, $zip, [System.IO.Compression.CompressionLevel]::Optimal, $true)

# ---------- 6. 결과 ----------
Write-Host ''
Write-Host "만들었습니다: $dest" -ForegroundColor Green
Get-ChildItem -LiteralPath $dest -Recurse -File | ForEach-Object {
  '{0,12:N0}  {1}' -f $_.Length, $_.FullName.Substring($dest.Length + 1)
}
Write-Host ('zip: {0} ({1:N0} 바이트)' -f $zip, (Get-Item -LiteralPath $zip).Length)
