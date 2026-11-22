<#
.SYNOPSIS
  전체 시험. 작업을 마칠 때(커밋·합치기 직전) 한 번만 돌린다(AGENTS.md "빌드·테스트"의 3단계).

.DESCRIPTION
  기본으로 도는 단계(앱 창·소리 없음, 이 PC의 실제 화면은 메모리로만 캡처):
    fmt      cargo fmt --all --check
    clippy   cargo clippy --workspace --all-targets --features full-tests
    cargo    cargo test-full (= cargo test --workspace --features full-tests --no-fail-fast)
             단위 시험 + 서버 E2E, 로컬 랜 방송 통합, 실제 화면 캡처·인코딩 시험
    check    apps/desktop: npm run check (svelte-check)
    vitest   apps/desktop: npm test
    e2e-lib  apps/desktop: node --test e2e/lib/*.test.mjs (CDP 하네스의 순수 함수)
    build    apps/desktop: npm run build
    shots    apps/desktop: npm run shots (화면 비교 하네스 34개, 헤드리스 Edge)
  -App을 주면 실제 앱 두 개를 띄우는 CDP 하네스(cargo build -p dongsi-desktop, npm run e2e -- --scenario all)도
  돌린다. 앱 창이 몇 분 동안 뜨고 사라진다. 사람이 PC를 쓰는 중이면 먼저 주인에게 알린다.
  -Linux를 주면 WSL Docker 리눅스 서버 E2E(scripts/e2e-linux-server.ps1)도 돌린다.

  단계가 실패해도 다음 단계로 넘어가고, 끝에 단계별 결과·시간 표를 출력한다. 하나라도 실패하면 종료 코드 1.
  로그와 summary.md는 target/test-full/<시각>/에 남는다. Windows PowerShell 5.1과 pwsh 7에서 모두 돈다.
  FFmpeg DLL 폴더를 PATH 앞에 붙인다. 순서: DONGSI_FFMPEG_BIN > FFMPEG_DIR의 bin > third_party/ffmpeg/bin.

.PARAMETER App
  실제 앱 CDP 통합 하네스(apps/desktop/e2e, 시나리오 all)를 더한다.
.PARAMETER Linux
  리눅스 서버 E2E(scripts/e2e-linux-server.ps1, 기본 인자)를 더한다.
.PARAMETER All
  -App -Linux와 같다.
.PARAMETER Skip
  건너뛸 단계 이름(쉼표로 구분): fmt, clippy, cargo, check, vitest, e2e-lib, build, shots, app, linux.

.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\test-full.ps1
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\test-full.ps1 -Skip shots,build
#>
[CmdletBinding()]
param(
    [switch]$App,
    [switch]$Linux,
    [switch]$All,
    [string[]]$Skip = @()
)

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }
try { $OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

if ($All) { $App = $true; $Linux = $true }
$SkipSet = @{}
foreach ($i in $Skip) { foreach ($p in ($i -split '[,\s]+')) { if ($p) { $SkipSet[$p.ToLowerInvariant()] = $true } } }
$Known = @('fmt', 'clippy', 'cargo', 'check', 'vitest', 'e2e-lib', 'build', 'shots', 'app', 'linux')
foreach ($k in $SkipSet.Keys) {
    if ($Known -notcontains $k) { Write-Host "[test-full] 모르는 단계 이름: $k (가능: $($Known -join ', '))"; exit 2 }
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Desktop = Join-Path $RepoRoot 'apps\desktop'
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$RunDir = Join-Path $RepoRoot ('target\test-full\' + $Stamp)
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$Results = New-Object System.Collections.ArrayList
$TotalWatch = [System.Diagnostics.Stopwatch]::StartNew()

function Say([string]$msg) { Write-Host ('[test-full] ' + $msg) }

# FFmpeg DLL(avcodec-62.dll 등)을 찾게 PATH 앞에 붙인다(사용자 PATH에 다른 ffmpeg 빌드가 있을 수 있다).
$FfmpegBin = $null
if ($env:DONGSI_FFMPEG_BIN) { $FfmpegBin = $env:DONGSI_FFMPEG_BIN }
elseif ($env:FFMPEG_DIR) { $FfmpegBin = Join-Path $env:FFMPEG_DIR 'bin' }
else { $FfmpegBin = Join-Path $RepoRoot 'third_party\ffmpeg\bin' }
if (Test-Path -LiteralPath (Join-Path $FfmpegBin 'avcodec-62.dll')) {
    $env:PATH = $FfmpegBin + ';' + $env:PATH
    Say "FFmpeg DLL: $FfmpegBin"
} else {
    Say "경고: $FfmpegBin 에 avcodec-62.dll이 없습니다. PATH의 FFmpeg를 씁니다(AGENTS.md '빌드 환경')."
}

# 명령 하나를 단계로 돌린다. 출력은 화면과 로그 파일에 같이 남긴다.
function Invoke-Step([string]$Name, [string]$Dir, [string]$Exe, [string[]]$ArgList, [string]$SkipKey = '') {
    if (-not $SkipKey) { $SkipKey = $Name }
    if ($SkipSet.ContainsKey($SkipKey)) {
        [void]$Results.Add([pscustomobject]@{ Name = $Name; Result = '건너뜀'; Seconds = 0; Note = '-Skip'; Log = '' })
        return
    }
    $log = Join-Path $RunDir ($Name + '.log')
    $shown = ($Exe + ' ' + ($ArgList -join ' ')).Trim()
    Say "[$Name] $shown"
    $lines = New-Object System.Collections.Generic.List[string]
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $code = $null
    $prev = $ErrorActionPreference
    Push-Location -LiteralPath $Dir
    try {
        # 네이티브 명령의 stderr(cargo 진행 상황)가 오류로 끊기지 않게 한다.
        $ErrorActionPreference = 'Continue'
        & $Exe @ArgList 2>&1 | ForEach-Object {
            $line = [string]$_
            Write-Host $line
            $lines.Add($line)
        }
        $code = $LASTEXITCODE
    } catch {
        $lines.Add('[test-full] 실행 실패: ' + $_)
        Write-Host ('[test-full] 실행 실패: ' + $_)
        $code = -1
    } finally {
        $ErrorActionPreference = $prev
        Pop-Location
    }
    $watch.Stop()
    [System.IO.File]::WriteAllLines($log, $lines, $Utf8NoBom)
    $note = ''
    if ($Name -eq 'cargo') {
        $passed = 0; $failed = 0; $ignored = 0; $seen = $false
        foreach ($l in $lines) {
            if ($l -match 'test result: \S+\. (\d+) passed; (\d+) failed; (\d+) ignored') {
                $seen = $true
                $passed += [int]$Matches[1]; $failed += [int]$Matches[2]; $ignored += [int]$Matches[3]
            }
        }
        if ($seen) { $note = "통과 $passed, 실패 $failed, 무시 $ignored" }
    }
    $result = if ($code -eq 0) { '통과' } else { "실패(종료 코드 $code)" }
    [void]$Results.Add([pscustomobject]@{ Name = $Name; Result = $result; Seconds = [math]::Round($watch.Elapsed.TotalSeconds); Note = $note; Log = $log })
    Say ("[$Name] $result, {0:N0}초" -f $watch.Elapsed.TotalSeconds)
}

$Npm = 'npm.cmd'
$Ps = (Get-Process -Id $PID).Path   # 지금 쓰는 PowerShell(powershell.exe 또는 pwsh.exe)

Invoke-Step 'fmt' $RepoRoot 'cargo' @('fmt', '--all', '--check')
Invoke-Step 'clippy' $RepoRoot 'cargo' @('clippy', '--workspace', '--all-targets', '--features', 'full-tests')
Invoke-Step 'cargo' $RepoRoot 'cargo' @('test-full')

if (-not (Test-Path -LiteralPath (Join-Path $Desktop 'node_modules'))) {
    Say "경고: $Desktop\node_modules가 없습니다. apps/desktop에서 npm install을 먼저 하세요."
}
Invoke-Step 'check' $Desktop $Npm @('run', 'check')
Invoke-Step 'vitest' $Desktop $Npm @('test')
$e2eLib = @(Get-ChildItem -LiteralPath (Join-Path $Desktop 'e2e\lib') -Filter '*.test.mjs' | Sort-Object Name | ForEach-Object { 'e2e/lib/' + $_.Name })
Invoke-Step 'e2e-lib' $Desktop 'node' (@('--test') + $e2eLib)
Invoke-Step 'build' $Desktop $Npm @('run', 'build')
Invoke-Step 'shots' $Desktop $Npm @('run', 'shots', '--', '--out', (Join-Path $RunDir 'shots'))

if ($App) {
    # CDP 하네스는 tauri CLI 없이 만든 디버그 exe(vite dev 서버 1420을 읽는 개발 모드)를 띄운다.
    Invoke-Step 'app-build' $RepoRoot 'cargo' @('build', '-p', 'dongsi-desktop') 'app'
    if ($Results[$Results.Count - 1].Result -eq '통과') {
        Invoke-Step 'app' $Desktop $Npm @('run', 'e2e', '--', '--scenario', 'all', '--out', (Join-Path $RunDir 'app-e2e'))
    }
}
if ($Linux) {
    Invoke-Step 'linux' $RepoRoot $Ps @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $PSScriptRoot 'e2e-linux-server.ps1'))
}

$TotalWatch.Stop()
$Commit = ''
try {
    $ErrorActionPreference = 'Continue'
    $Commit = [string](& git -C $RepoRoot rev-parse --short HEAD 2>$null)
} catch { } finally { $ErrorActionPreference = 'Stop' }
$md = New-Object System.Collections.Generic.List[string]
$md.Add('# 전체 시험 ' + $Stamp)
$md.Add('')
$md.Add(('전체 {0:N0}초. 커밋 {1}' -f $TotalWatch.Elapsed.TotalSeconds, $Commit))
$md.Add('')
$md.Add('| 단계 | 결과 | 초 | 비고 | 로그 |')
$md.Add('|---|---|---|---|---|')
foreach ($r in $Results) { $md.Add("| $($r.Name) | $($r.Result) | $($r.Seconds) | $($r.Note) | $($r.Log) |") }
if (-not $App) { $md.Add(''); $md.Add('실제 앱 CDP 하네스는 돌리지 않았다(-App).') }
if (-not $Linux) { $md.Add('리눅스 서버 E2E는 돌리지 않았다(-Linux).') }
[System.IO.File]::WriteAllLines((Join-Path $RunDir 'summary.md'), $md, $Utf8NoBom)

Write-Host ''
Say ('요약 (전체 {0:N0}초)' -f $TotalWatch.Elapsed.TotalSeconds)
$Results | Format-Table -AutoSize Name, Result, Seconds, Note | Out-String -Width 200 | Write-Host
Say ('결과: ' + (Join-Path $RunDir 'summary.md'))
$bad = @($Results | Where-Object { $_.Result -like '실패*' })
if ($bad.Count -gt 0) { exit 1 }
exit 0
