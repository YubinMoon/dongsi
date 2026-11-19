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
param(
  [string]$FfmpegDir = $env:FFMPEG_DIR,
  [switch]$Debug,
  [switch]$SkipBuild,
  [string]$OutDir
)

$ErrorActionPreference = 'Stop'
$nl = [Environment]::NewLine
$repo = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $repo 'dist' }

function Fail([string]$msg) { Write-Host "오류: $msg" -ForegroundColor Red; exit 1 }

# ---------- 1. FFmpeg 폴더 확인(GPL·nonfree면 멈춘다) ----------
if (-not $FfmpegDir) { Fail 'FFmpeg 폴더를 알 수 없습니다. -FfmpegDir <폴더>를 주거나 환경 변수 FFMPEG_DIR을 설정하세요.' }
if (-not (Test-Path -LiteralPath $FfmpegDir -PathType Container)) { Fail "FFmpeg 폴더가 없습니다: $FfmpegDir" }
$FfmpegDir = (Resolve-Path -LiteralPath $FfmpegDir).Path
$licDir = Join-Path $FfmpegDir 'licenses'
$manifestPath = Join-Path $licDir 'components.json'
if (-not (Test-Path -LiteralPath $manifestPath)) {
  Fail "FFmpeg 폴더에 licenses\components.json이 없습니다: $manifestPath (LGPL 빌드가 아니거나 scripts/ffmpeg/build.sh의 결과가 아닙니다)"
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$configure = [string]$manifest.ffmpeg.configure
if (-not $configure) { Fail 'components.json에 ffmpeg.configure가 없습니다.' }
foreach ($bad in '--enable-gpl', '--enable-nonfree', '--enable-libx264', '--enable-libx265', '--enable-libxvid') {
  if (($configure -split '\s+') -contains $bad) { Fail "FFmpeg configure에 $bad 가 있습니다. GPL·nonfree 빌드는 배포할 수 없습니다(D-062)." }
}
$bundle = $manifest.sourceBundle
if (-not $bundle -or -not $bundle.file -or $bundle.sha256 -notmatch '^[0-9a-f]{64}$') { Fail 'components.json의 sourceBundle(file, sha256)이 올바르지 않습니다.' }
$ffBin = Join-Path $FfmpegDir 'bin'
$dlls = @(Get-ChildItem -LiteralPath $ffBin -Filter '*.dll' -File | Sort-Object Name)
if ($dlls.Count -eq 0) { Fail "FFmpeg 폴더의 bin\에 DLL이 없습니다: $ffBin" }
if (-not (Test-Path -LiteralPath (Join-Path $repo 'THIRD_PARTY_NOTICES.md'))) { Fail 'THIRD_PARTY_NOTICES.md가 없습니다. node scripts/licenses/generate.mjs로 먼저 만드세요.' }

# ---------- 2. 빌드 ----------
$config = if ($Debug) { 'debug' } else { 'release' }
$suffix = if ($Debug) { '-debug' } else { '' }
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$exe = Join-Path $targetDir "$config\dongsi-desktop.exe"

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
if (-not (Test-Path -LiteralPath $exe)) { Fail "exe가 없습니다: $exe (CARGO_TARGET_DIR을 확인하세요. -SkipBuild 없이 다시 실행해도 됩니다)" }

# ---------- 3. exe가 요구하는 DLL 점검(FFmpeg DLL과 시스템 DLL 말고는 없어야 한다) ----------
function Find-ObjDump {
  $cands = @()
  if ($env:LIBCLANG_PATH) { $cands += Join-Path $env:LIBCLANG_PATH 'llvm-objdump.exe' }
  $cands += (Get-Command 'llvm-objdump.exe' -ErrorAction SilentlyContinue | ForEach-Object { $_.Source })
  foreach ($c in $cands) { if ($c -and (Test-Path -LiteralPath $c)) { return $c } }
  return $null
}
$objdump = Find-ObjDump
if ($objdump) {
  $imports = @(& $objdump -p $exe | Select-String 'DLL Name:' | ForEach-Object { ($_ -split 'DLL Name:')[1].Trim() } | Sort-Object -Unique)
  $ffNames = @($dlls | ForEach-Object { $_.Name.ToLowerInvariant() })
  $sys = Join-Path $env:windir 'System32'
  $extra = @()
  foreach ($d in $imports) {
    $l = $d.ToLowerInvariant()
    if ($l -like 'api-ms-win-*' -or $l -like 'ext-ms-win-*') { continue }
    if ($ffNames -contains $l) { continue }
    if (Test-Path -LiteralPath (Join-Path $sys $d)) { continue }
    $extra += $d
  }
  if ($extra.Count -gt 0) {
    Write-Host ("경고: exe가 시스템에 없는 DLL을 요구합니다: " + ($extra -join ', ') + " — 포터블 폴더에 같이 넣어야 하는지 확인하세요.") -ForegroundColor Yellow
  } else {
    Write-Host "[점검] exe가 요구하는 DLL은 FFmpeg DLL과 시스템 DLL뿐입니다 ($($imports.Count)개)." -ForegroundColor Green
  }
} else {
  Write-Host '경고: llvm-objdump를 찾지 못해 exe의 DLL 요구를 점검하지 못했습니다(LIBCLANG_PATH에 LLVM bin을 두세요).' -ForegroundColor Yellow
}

# ---------- 4. 폴더 만들기 ----------
$ver = (Get-Content -LiteralPath (Join-Path $repo 'apps\desktop\src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json).version
$name = "dongsi-$ver-windows-x64-portable$suffix"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$dest = Join-Path $OutDir $name
$zip = "$dest.zip"
# 지우기 전에 결과 폴더가 OutDir 바로 아래이고 링크가 없는지 확인한다(정션을 따라가 다른 곳을 지우지 않게).
if (Test-Path -LiteralPath $dest) {
  if ((Split-Path -Parent $dest) -ne $OutDir) { Fail "지우려는 폴더가 예상 위치가 아닙니다: $dest" }
  $links = @(Get-ChildItem -LiteralPath $dest -Recurse -Force -Attributes ReparsePoint -ErrorAction SilentlyContinue)
  if ($links.Count -gt 0) { Fail "기존 결과 폴더에 링크가 있어 지우지 않습니다: $($links[0].FullName)" }
  Remove-Item -LiteralPath $dest -Recurse -Force
}
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
New-Item -ItemType Directory -Force -Path $dest | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $dest 'dongsi.exe')
foreach ($d in $dlls) { Copy-Item -LiteralPath $d.FullName -Destination $dest }
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $dest
Copy-Item -LiteralPath (Join-Path $repo 'THIRD_PARTY_NOTICES.md') -Destination $dest
$destLic = Join-Path $dest 'licenses'
Copy-Item -LiteralPath $licDir -Destination $destLic -Recurse
$buildInfo = Join-Path $FfmpegDir 'BUILDINFO.md'
if (Test-Path -LiteralPath $buildInfo) { Copy-Item -LiteralPath $buildInfo -Destination (Join-Path $destLic 'FFmpeg-BUILDINFO.md') }
Copy-Item -LiteralPath (Join-Path $repo 'apps\desktop\static\licenses\Pretendard-OFL-1.1.txt') -Destination $destLic

$dllList = ($dlls | ForEach-Object { '  ' + $_.Name }) -join $nl
$debugNote = ''
if ($Debug) { $debugNote = $nl + '※ 이 폴더는 디버그 빌드입니다. 테스트 패턴 방송 같은 개발용 기능이 들어 있고, 배포용이 아닙니다.' + $nl }
$readme = @"
dongsi $ver 포터블 (Windows x64)
================================
$debugNote
설치 파일이 아닙니다. 폴더 전체를 원하는 곳에 풀고 dongsi.exe를 실행하세요.

[실행]
- dongsi.exe를 실행합니다. Windows 11에는 WebView2 런타임이 들어 있습니다(없으면 Microsoft에서 설치).
- dongsi.exe 옆의 DLL 파일(아래 목록)은 FFmpeg입니다. exe와 같은 폴더에 있어야 실행됩니다.
$dllList
- 앱 데이터(설정, 서버 목록)는 %APPDATA%\com.dongsi.desktop\ 에 저장됩니다. 지우면 처음 실행한 상태가 됩니다.
- 기본 포트는 7884입니다(TCP: 시그널링, UDP: 미디어와 로컬 감지). 방화벽에서 허용해야 할 수 있습니다.

[라이선스]
- dongsi는 MIT 라이선스입니다(LICENSE 파일).
- 이 프로그램은 LGPL 2.1 이상으로 배포되는 FFmpeg를 사용합니다. FFmpeg는 GPL·nonfree 부품 없이 만든 LGPL 빌드이고, 공유 라이브러리(DLL)로 연결됩니다.
  FFmpeg는 FFmpeg 개발자들의 소프트웨어이며 dongsi가 소유한 코드가 아닙니다. 프로젝트: https://ffmpeg.org/
- 그 밖의 제3자 소프트웨어(FFmpeg 안의 라이브러리, 글꼴, Rust·npm 라이브러리)의 라이선스는 THIRD_PARTY_NOTICES.md와 licenses 폴더에 있습니다.
  앱의 "설정 > 이 앱 > 제3자 고지 > 보기"에서도 볼 수 있습니다.

[FFmpeg 소스]
- FFmpeg 소스 묶음(이 DLL을 만든 소스, 빌드 방법 포함)은 https://github.com/YubinMoon/dongsi/releases 의 릴리스 첨부 파일로 있습니다.
  파일 이름: $($bundle.file)
  SHA-256: $($bundle.sha256)
- 같은 DLL을 다시 만드는 방법은 저장소(https://github.com/YubinMoon/dongsi)의 scripts/ffmpeg 폴더와 docs/licensing.md에 있습니다.

[FFmpeg DLL 바꾸기]
- LGPL에 따라 FFmpeg DLL을 수정한 버전이나 호환되는 다른 빌드로 바꿀 수 있습니다.
- 소스 묶음으로 직접 만든 DLL(이름과 FFmpeg 주 버전이 같아야 함)을 dongsi.exe 옆의 같은 이름 파일에 덮어쓰면 됩니다. 앱을 다시 만들 필요가 없습니다.
- DLL의 이름이나 ABI가 다르면 앱이 시작하지 못할 수 있습니다. 문제가 생기면 원래 DLL로 되돌리세요.
"@
[System.IO.File]::WriteAllText((Join-Path $dest 'README.txt'), $readme, (New-Object System.Text.UTF8Encoding($true)))

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
