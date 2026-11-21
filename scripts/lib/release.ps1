# scripts/package-portable.ps1과 scripts/package-installer.ps1이 같이 쓰는 함수. Windows PowerShell 5.1 이상.
# 점 소싱해서 쓴다:  . (Join-Path $PSScriptRoot 'lib\release.ps1')
# 라이선스 의무와 릴리스 절차는 docs/licensing.md를 본다.

function Fail([string]$msg) { Write-Host "오류: $msg" -ForegroundColor Red; exit 1 }

# FFmpeg LGPL 빌드 폴더(scripts/ffmpeg/build.sh의 결과)를 확인한다. GPL·nonfree 빌드면 멈춘다(D-062).
# 돌려주는 값: Dir, LicDir, Bin, Dlls(FileInfo 배열, 이름순), Bundle(components.json의 sourceBundle), BuildInfo(없으면 $null)
function Get-LgplFfmpeg([string]$FfmpegDir, [string]$Repo) {
  if (-not $FfmpegDir) { Fail 'FFmpeg 폴더를 알 수 없습니다. -FfmpegDir <폴더>를 주거나 환경 변수 FFMPEG_DIR을 설정하세요.' }
  if (-not (Test-Path -LiteralPath $FfmpegDir -PathType Container)) { Fail "FFmpeg 폴더가 없습니다: $FfmpegDir" }
  $dir = (Resolve-Path -LiteralPath $FfmpegDir).Path
  $licDir = Join-Path $dir 'licenses'
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
  $bin = Join-Path $dir 'bin'
  $dlls = @(Get-ChildItem -LiteralPath $bin -Filter '*.dll' -File | Sort-Object Name)
  if ($dlls.Count -eq 0) { Fail "FFmpeg 폴더의 bin\에 DLL이 없습니다: $bin" }
  if (-not (Test-Path -LiteralPath (Join-Path $Repo 'THIRD_PARTY_NOTICES.md'))) { Fail 'THIRD_PARTY_NOTICES.md가 없습니다. node scripts/licenses/generate.mjs로 먼저 만드세요.' }
  $buildInfo = Join-Path $dir 'BUILDINFO.md'
  if (-not (Test-Path -LiteralPath $buildInfo)) { $buildInfo = $null }
  [pscustomobject]@{ Dir = $dir; LicDir = $licDir; Bin = $bin; Dlls = $dlls; Bundle = $bundle; BuildInfo = $buildInfo }
}

# 앱 버전(tauri.conf.json의 version)
function Get-AppVersion([string]$Repo) {
  (Get-Content -LiteralPath (Join-Path $Repo 'apps\desktop\src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json).version
}

# tauri build가 만든 exe. tauri.conf.json의 mainBinaryName(dongsi)으로 이름이 바뀐다.
# cargo build만 했으면 크레이트 이름(dongsi-desktop.exe)이다.
function Get-AppExe([string]$TargetDir, [string]$Config, [string]$Repo) {
  $conf = Get-Content -LiteralPath (Join-Path $Repo 'apps\desktop\src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json
  $names = @()
  if ($conf.mainBinaryName) { $names += [string]$conf.mainBinaryName }
  $names += 'dongsi-desktop'
  foreach ($n in $names) {
    $p = Join-Path $TargetDir "$Config\$n.exe"
    if (Test-Path -LiteralPath $p) { return $p }
  }
  Join-Path $TargetDir "$Config\$($names[0]).exe"
}

# cargo target 폴더(CARGO_TARGET_DIR을 따른다)
function Get-TargetDir([string]$Repo) {
  if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' }
}

# exe와 FFmpeg DLL이 요구하는 DLL이 서로와 시스템 DLL뿐인지 점검한다(깨끗한 PC에서 실행되는지).
# llvm-objdump가 없으면 경고만 한다.
function Test-DllImports([string]$Exe, $Dlls) {
  $cands = @()
  if ($env:LIBCLANG_PATH) { $cands += Join-Path $env:LIBCLANG_PATH 'llvm-objdump.exe' }
  $cands += (Get-Command 'llvm-objdump.exe' -ErrorAction SilentlyContinue | ForEach-Object { $_.Source })
  $objdump = $null
  foreach ($c in $cands) { if ($c -and (Test-Path -LiteralPath $c)) { $objdump = $c; break } }
  if (-not $objdump) {
    Write-Host '경고: llvm-objdump를 찾지 못해 DLL 요구를 점검하지 못했습니다(LIBCLANG_PATH에 LLVM bin을 두세요).' -ForegroundColor Yellow
    return
  }
  $ffNames = @($Dlls | ForEach-Object { $_.Name.ToLowerInvariant() })
  $sys = Join-Path $env:windir 'System32'
  $problems = @()
  foreach ($f in @($Exe) + @($Dlls | ForEach-Object { $_.FullName })) {
    # llvm-objdump -p의 "The Import Tables:" 절만 읽는다(Export Table의 DLL Name은 자기 이름이다).
    $inImports = $false
    $imports = @(& $objdump -p $f | ForEach-Object {
      if ($_ -match 'The Import Tables') { $inImports = $true }
      elseif ($_ -match '^(Export Table|The Delay Import Tables|The Resource)') { $inImports = $false }
      elseif ($inImports -and $_ -match 'DLL Name:\s*(\S+)') { $Matches[1] }
    } | Sort-Object -Unique)
    foreach ($d in $imports) {
      $l = $d.ToLowerInvariant()
      if ($l -like 'api-ms-win-*' -or $l -like 'ext-ms-win-*') { continue }
      if ($ffNames -contains $l) { continue }
      if (Test-Path -LiteralPath (Join-Path $sys $d)) { continue }
      $problems += ((Split-Path -Leaf $f) + ' -> ' + $d)
    }
  }
  if ($problems.Count -gt 0) {
    Fail ("시스템에 없고 FFmpeg bin에도 없는 DLL을 요구합니다(깨끗한 PC에서 실행되지 않음): " + ($problems -join ', '))
  }
  Write-Host ("[점검] exe와 FFmpeg DLL " + @($Dlls).Count + "개가 요구하는 DLL은 서로와 시스템 DLL뿐입니다.") -ForegroundColor Green
}

# 지울 폴더가 Parent 바로 아래이고 링크(정션 등)가 없을 때만 지운다(정션을 따라가 다른 곳을 지우지 않게).
function Remove-OutputDir([string]$Path, [string]$Parent) {
  if (-not (Test-Path -LiteralPath $Path)) { return }
  if ((Split-Path -Parent $Path) -ne $Parent) { Fail "지우려는 폴더가 예상 위치가 아닙니다: $Path" }
  $links = @(Get-ChildItem -LiteralPath $Path -Recurse -Force -Attributes ReparsePoint -ErrorAction SilentlyContinue)
  if ($links.Count -gt 0) { Fail "폴더에 링크가 있어 지우지 않습니다: $($links[0].FullName)" }
  Remove-Item -LiteralPath $Path -Recurse -Force
}

# 배포물에 함께 넣는 라이선스 파일: LICENSE, THIRD_PARTY_NOTICES.md, licenses\(FFmpeg 구성 요소, FFmpeg-BUILDINFO.md, Pretendard)
function Copy-LicenseFiles([string]$Repo, $Ff, [string]$Dest) {
  Copy-Item -LiteralPath (Join-Path $Repo 'LICENSE') -Destination $Dest
  Copy-Item -LiteralPath (Join-Path $Repo 'THIRD_PARTY_NOTICES.md') -Destination $Dest
  $destLic = Join-Path $Dest 'licenses'
  Copy-Item -LiteralPath $Ff.LicDir -Destination $destLic -Recurse
  if ($Ff.BuildInfo) { Copy-Item -LiteralPath $Ff.BuildInfo -Destination (Join-Path $destLic 'FFmpeg-BUILDINFO.md') }
  Copy-Item -LiteralPath (Join-Path $Repo 'apps\desktop\static\licenses\Pretendard-OFL-1.1.txt') -Destination $destLic
}

# README.txt(UTF-8 BOM). Kind: portable(zip을 풀어 쓰는 폴더) / installer(설치 폴더)
function Write-ReleaseReadme([string]$Path, [string]$Version, $Ff, [string]$Kind, [bool]$DebugBuild = $false) {
  $nl = [Environment]::NewLine
  $dllList = ($Ff.Dlls | ForEach-Object { '  ' + $_.Name }) -join $nl
  $bundle = $Ff.Bundle
  $debugNote = ''
  if ($DebugBuild) { $debugNote = $nl + '※ 디버그 빌드입니다. 테스트 패턴 방송 같은 개발용 기능이 들어 있고, 배포용이 아닙니다.' + $nl }
  if ($Kind -eq 'installer') {
    $title = "dongsi $Version (Windows x64)"
    $intro = '설치 파일(dongsi-' + $Version + '-windows-x64-setup.exe)로 설치한 폴더입니다. 기본 위치는 %LOCALAPPDATA%\dongsi 입니다.'
    $run = '- 시작 메뉴의 "동시"를 실행합니다(윈도우 검색에 "동시"나 "dongsi"를 입력해도 됩니다). Windows 11에는 WebView2 런타임이 들어 있습니다(없으면 설치 중에 받습니다).' + $nl +
           '- 이 폴더의 DLL 파일(아래 목록)은 FFmpeg입니다. dongsi.exe와 같은 폴더에 있어야 실행됩니다.'
    $place = '설치 폴더의 같은 이름 파일에 덮어쓰면 됩니다. 앱을 다시 만들 필요가 없습니다. 앱을 다시 설치하거나 새 버전으로 올리면 원래 DLL로 돌아갑니다.'
  } else {
    $title = "dongsi $Version 포터블 (Windows x64)"
    $intro = '설치 파일이 아닙니다. 폴더 전체를 원하는 곳에 풀고 dongsi.exe를 실행하세요.'
    $run = '- dongsi.exe를 실행합니다. Windows 11에는 WebView2 런타임이 들어 있습니다(없으면 Microsoft에서 설치).' + $nl +
           '- dongsi.exe 옆의 DLL 파일(아래 목록)은 FFmpeg입니다. exe와 같은 폴더에 있어야 실행됩니다.'
    $place = 'dongsi.exe 옆의 같은 이름 파일에 덮어쓰면 됩니다. 앱을 다시 만들 필요가 없습니다.'
  }
  $readme = @"
$title
$('=' * 32)
$debugNote
$intro

[실행]
$run
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
- 소스 묶음으로 직접 만든 DLL(이름과 FFmpeg 주 버전이 같아야 함)을 $place
- DLL의 이름이나 ABI가 다르면 앱이 시작하지 못할 수 있습니다. 문제가 생기면 원래 DLL로 되돌리세요.
"@
  [System.IO.File]::WriteAllText($Path, $readme, (New-Object System.Text.UTF8Encoding($true)))
}
