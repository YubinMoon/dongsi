<#
.SYNOPSIS
  리눅스 서버(네이티브·Docker)를 상대로 클라이언트-서버 통신 E2E 시험을 돌린다. Windows에서 실행한다.

.DESCRIPTION
  커밋한 상태(HEAD)를 WSL로 옮겨(git bundle) 서버를 빌드하고, 모드(native, bridge, host, remap)마다 서버를
  새로 띄운 뒤 Windows·WSL 클라이언트가 crates/server/tests/e2e 시험을 돌린다.
  결과는 target/e2e-linux-server/<시각>/ 아래에 로그와 summary.md로 남는다. 하나라도 실패하면 종료 코드 1.
  Windows PowerShell 5.1과 pwsh 7에서 모두 돈다. 자세한 안내: deploy/e2e-test.md

.PARAMETER Modes
  서버를 띄우는 방식. native(WSL의 네이티브 프로세스), bridge(Docker 브리지, 같은 번호 매핑),
  host(Docker host 네트워크), remap(Docker 브리지, 호스트 포트와 컨테이너 포트가 다름). 쉼표로 구분.
.PARAMETER Clients
  시험을 돌리는 쪽. windows(127.0.0.1과 LAN IP 각각), wsl(127.0.0.1). 쉼표로 구분.
.PARAMETER WslDir
  WSL 쪽 클론 위치(기본 ~/project/dongsi/e2e). 공백이 없는 경로. 네이티브 서버 데이터는 <WslDir>-data/native.
.PARAMETER PortBase
  포트 시작 번호(기본 8310). native=+0, bridge=+2, host=+4, remap=호스트 +6 -> 컨테이너 +7. 7884 근처는 거부한다.
.PARAMETER LanIp
  Windows 클라이언트가 쓸 LAN IP. 기본은 기본 경로 인터페이스의 IPv4.
.PARAMETER SkipBuild
  빌드를 건너뛴다(이미 빌드한 이미지·바이너리·시험 파일을 쓴다).
.PARAMETER SkipDestructive
  파괴적 시험(rate_limit, server_restart)을 건너뛴다.
.PARAMETER KeepServers
  끝나도 서버(컨테이너, 네이티브 프로세스)를 끄지 않는다.
.PARAMETER ImageTag
  Docker 이미지 태그(기본 dongsi-server:e2e). 이미지는 끝나도 남긴다.
.PARAMETER Prefix
  컨테이너·볼륨 이름 접두사(기본 dongsi-e2e). 이름은 <Prefix>-<모드>. 이 이름의 것만 지우고 만든다.

.EXAMPLE
  powershell.exe -NoProfile -File scripts\e2e-linux-server.ps1 -Modes native -Clients windows
#>
[CmdletBinding()]
param(
    [string[]]$Modes = @('native', 'bridge', 'host', 'remap'),
    [string[]]$Clients = @('windows', 'wsl'),
    [string]$WslDir = '~/project/dongsi/e2e',
    [int]$PortBase = 8310,
    [string]$LanIp = '',
    [switch]$SkipBuild,
    [switch]$SkipDestructive,
    [switch]$KeepServers,
    [string]$ImageTag = 'dongsi-server:e2e',
    [string]$Prefix = 'dongsi-e2e'
)

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }
try { $OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }
$env:WSL_UTF8 = '1'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$RunDir = Join-Path $RepoRoot ('target\e2e-linux-server\' + $Stamp)
$TotalWatch = [System.Diagnostics.Stopwatch]::StartNew()
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$LF = [string][char]10

$Results = New-Object System.Collections.ArrayList
$Started = New-Object System.Collections.ArrayList   # 우리가 띄운 서버(정리 대상)
$script:WslScriptCount = 0
$script:Invites = @{}

# ---------------------------------------------------------------------------
# 도우미

function Say([string]$msg) { Write-Host ('[e2e] ' + $msg) }
function Warn([string]$msg) { Write-Host ('[e2e] 경고: ' + $msg) -ForegroundColor Yellow }
function Fail([string]$msg) { throw $msg }

function Split-List([string[]]$items) {
    $out = @()
    foreach ($i in $items) {
        foreach ($p in ($i -split '[,\s]+')) { if ($p) { $out += $p.ToLowerInvariant() } }
    }
    return ,$out
}

# Windows 명령줄 인자 하나를 따옴표로 감싼다.
function Quote-Win([string]$s) {
    if ($s -eq '') { return '""' }
    if ($s -notmatch '[\s"]') { return $s }
    return '"' + ($s -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
}

# bash 작은따옴표 문자열.
function Q([string]$s) { return "'" + $s.Replace("'", "'\''") + "'" }

function ConvertTo-WslPath([string]$winPath) {
    $full = [System.IO.Path]::GetFullPath($winPath)
    $drive = $full.Substring(0, 1).ToLowerInvariant()
    return '/mnt/' + $drive + ($full.Substring(2) -replace '\\', '/')
}

# 환경 변수를 잠깐 바꿔서 블록을 실행한다.
function With-Env([hashtable]$vars, [scriptblock]$body) {
    $old = @{}
    foreach ($k in $vars.Keys) {
        $old[$k] = [System.Environment]::GetEnvironmentVariable($k, 'Process')
        [System.Environment]::SetEnvironmentVariable($k, [string]$vars[$k], 'Process')
    }
    try { return (& $body) }
    finally {
        foreach ($k in $vars.Keys) { [System.Environment]::SetEnvironmentVariable($k, $old[$k], 'Process') }
    }
}

# 프로그램을 실행해 출력(표준 출력+오류)을 로그 파일에 모은다. 시간 제한을 넘으면 프로세스 트리를 죽인다.
function Invoke-Proc([string]$exe, [string[]]$argList, [string]$logFile, [int]$timeoutSec = 600, [string]$workDir = '') {
    $inner = (Quote-Win $exe)
    foreach ($a in $argList) { $inner += ' ' + (Quote-Win $a) }
    $inner += ' < NUL > ' + (Quote-Win $logFile) + ' 2>&1'
    $cmdArgs = '/d /s /c "' + $inner + '"'
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $sp = @{ FilePath = $env:ComSpec; ArgumentList = $cmdArgs; NoNewWindow = $true; PassThru = $true }
    if ($workDir) { $sp['WorkingDirectory'] = $workDir }
    $p = Start-Process @sp
    $null = $p.Handle
    $timedOut = $false
    if (-not $p.WaitForExit($timeoutSec * 1000)) {
        $timedOut = $true
        & taskkill.exe /PID $p.Id /T /F 2>&1 | Out-Null
        $null = $p.WaitForExit(10000)
    }
    $sw.Stop()
    $code = -1
    if (-not $timedOut) { $p.WaitForExit(); $code = $p.ExitCode }
    $text = ''
    if (Test-Path -LiteralPath $logFile) { $text = [System.IO.File]::ReadAllText($logFile, [System.Text.Encoding]::UTF8) }
    return [pscustomobject]@{ ExitCode = $code; Seconds = $sw.Elapsed.TotalSeconds; TimedOut = $timedOut; Text = $text; Log = $logFile }
}

# WSL에서 bash 스크립트를 실행한다. 스크립트는 파일로 써서(따옴표 문제를 피하려고) bash에 준다.
function Invoke-Wsl([string]$label, [string]$body, [int]$timeoutSec = 600) {
    $script:WslScriptCount++
    $name = ('{0:D2}-{1}' -f $script:WslScriptCount, $label)
    $file = Join-Path $RunDir ($name + '.sh')
    $header = 'set -u' + $LF + '[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"' + $LF
    if ($env:DONGSI_STOP_SIGNAL) { $header += 'export DONGSI_STOP_SIGNAL=' + (Q $env:DONGSI_STOP_SIGNAL) + $LF }
    $text = ($header + $body) -replace '\r\n', $LF
    [System.IO.File]::WriteAllText($file, $text, $Utf8NoBom)
    $log = Join-Path $RunDir ($name + '.log')
    return (Invoke-Proc 'wsl.exe' @('-e', 'bash', (ConvertTo-WslPath $file)) $log $timeoutSec)
}

function Last-Invite([string]$text) {
    $m = [regex]::Matches($text, 'dongsi1:[A-Za-z0-9_-]+')
    if ($m.Count -eq 0) { return $null }
    return $m[$m.Count - 1].Value
}

function Tail([string]$text, [int]$n = 15) {
    $lines = $text -split '\r?\n'
    if ($lines.Count -le $n) { return ($lines -join $LF) }
    return (($lines | Select-Object -Last $n) -join $LF)
}

function First-Line([string]$text) { return ($text -split '\r?\n')[0] }

function Get-DefaultLanIp {
    try {
        $routes = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction Stop |
            Where-Object { $_.NextHop -ne '0.0.0.0' }
        $best = $null
        $bestMetric = [int]::MaxValue
        foreach ($r in $routes) {
            $ifm = (Get-NetIPInterface -InterfaceIndex $r.InterfaceIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue)
            $m = [int]$r.RouteMetric
            if ($ifm) { $m += [int]$ifm.InterfaceMetric }
            if ($m -lt $bestMetric) { $bestMetric = $m; $best = $r }
        }
        if ($best) {
            $ip = Get-NetIPAddress -InterfaceIndex $best.InterfaceIndex -AddressFamily IPv4 -ErrorAction Stop |
                Where-Object { $_.IPAddress -notlike '169.254.*' } | Select-Object -First 1
            if ($ip) { return $ip.IPAddress }
        }
    } catch { }
    return ''
}

# ---------------------------------------------------------------------------
# 매개변수 정리

$Modes = Split-List $Modes
$Clients = Split-List $Clients
foreach ($m in $Modes) { if (@('native', 'bridge', 'host', 'remap') -notcontains $m) { Fail "알 수 없는 모드: $m (native, bridge, host, remap)" } }
foreach ($c in $Clients) { if (@('windows', 'wsl') -notcontains $c) { Fail "알 수 없는 클라이언트: $c (windows, wsl)" } }
if ($Modes.Count -eq 0) { Fail '-Modes가 비었습니다' }
if ($Clients.Count -eq 0) { Fail '-Clients가 비었습니다' }
if ($PortBase -lt 1024 -or $PortBase + 7 -gt 65535) { Fail "-PortBase가 범위를 벗어났습니다: $PortBase" }
if ($PortBase + 7 -ge 7864 -and $PortBase -le 7904) { Fail "-PortBase $PortBase 는 기본 포트 7884 근처입니다. 7884는 사람이 쓰므로 자동 시험에서 쓰지 않습니다" }
if ($WslDir -match '\s') { Fail '-WslDir에 공백이 있으면 안 됩니다' }
if ($WslDir -match '^[A-Za-z]:' -or $WslDir -match '\\') { Fail "-WslDir는 WSL 안의 리눅스 경로여야 합니다(받은 값: $WslDir). Git Bash 등이 ~를 Windows 경로로 바꿨다면 '~/project/dongsi/e2e'처럼 따옴표로 감싸세요" }

$DockerModes = @($Modes | Where-Object { $_ -ne 'native' })
$HasDocker = $DockerModes.Count -gt 0
$HasNative = $Modes -contains 'native'
$HasWinClient = $Clients -contains 'windows'
$HasWslClient = $Clients -contains 'wsl'

$ModePort = @{ native = $PortBase; bridge = $PortBase + 2; host = $PortBase + 4; remap = $PortBase + 6 }   # 클라이언트가 접속하는 포트
$ModeCPort = @{ native = $PortBase; bridge = $PortBase + 2; host = $PortBase + 4; remap = $PortBase + 7 } # 서버가 듣는 포트

if ($HasWinClient -and -not $LanIp) {
    $LanIp = Get-DefaultLanIp
    if (-not $LanIp) { Warn 'LAN IP를 찾지 못해 Windows 클라이언트의 LAN 주소 시험은 건너뜁니다(-LanIp로 줄 수 있습니다)' }
}

New-Item -ItemType Directory -Force -Path $RunDir | Out-Null

function Container-Name([string]$mode) { return $Prefix + '-' + $mode }

# ---------------------------------------------------------------------------
# 본체

$exitCode = 0
$Commit = ''
$WslHome = ''
$WslDirAbs = ''
$WslData = ''
$WslBin = ''
$WslHelper = ''
try {
    Say "결과 폴더: $RunDir"

    # ---- 사전 확인 -------------------------------------------------------
    Say '사전 확인'
    $missing = @()
    if (-not (Get-Command 'wsl.exe' -ErrorAction SilentlyContinue)) {
        Fail 'wsl.exe가 없습니다. WSL2와 Ubuntu를 설치하세요(wsl --install). 자세한 안내: deploy/e2e-test.md'
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { $missing += 'Windows git' }
    if ($HasWinClient -and -not (Get-Command cargo -ErrorAction SilentlyContinue)) { $missing += 'Windows cargo(rustup: https://rustup.rs)' }

    $probe = Invoke-Wsl 'probe' @'
echo "HOME=$HOME"
command -v git >/dev/null 2>&1 && echo "OK:git" || echo "MISSING:git"
command -v docker >/dev/null 2>&1 && echo "OK:docker" || echo "MISSING:docker"
if command -v docker >/dev/null 2>&1; then docker info >/dev/null 2>&1 && echo "OK:docker-daemon" || echo "MISSING:docker-daemon"; fi
command -v cargo >/dev/null 2>&1 && echo "OK:cargo" || echo "MISSING:cargo"
command -v setsid >/dev/null 2>&1 && echo "OK:setsid" || echo "MISSING:setsid"
'@ 60
    if ($probe.ExitCode -ne 0) { Fail ('WSL을 실행하지 못했습니다(종료 코드 {0}). 로그: {1}{2}{3}' -f $probe.ExitCode, $probe.Log, $LF, (Tail $probe.Text)) }
    $hm = [regex]::Match($probe.Text, '(?m)^HOME=(\S+)')
    if (-not $hm.Success) { Fail 'WSL의 HOME을 읽지 못했습니다' }
    $WslHome = $hm.Groups[1].Value
    if ($probe.Text -match 'MISSING:git') { $missing += 'WSL git (sudo apt install git)' }
    if ($HasDocker -and $probe.Text -match 'MISSING:docker(\r?\n|$)') { $missing += 'WSL Docker Engine (deploy/e2e-test.md의 설치 예시)' }
    if ($HasDocker -and $probe.Text -match 'MISSING:docker-daemon') { $missing += 'WSL Docker 데몬 접근(데몬을 켜고, 사용자를 docker 그룹에 넣으세요)' }
    if (($HasNative -or $HasWslClient) -and $probe.Text -match 'MISSING:cargo') { $missing += 'WSL rustup/cargo (curl https://sh.rustup.rs -sSf | sh)' }
    if ($HasNative -and $probe.Text -match 'MISSING:setsid') { $missing += 'WSL setsid (util-linux)' }
    if ($missing.Count -gt 0) {
        Fail ('준비물이 없습니다:' + $LF + '  - ' + ($missing -join ($LF + '  - ')) + $LF + '자세한 안내: deploy/e2e-test.md')
    }

    if ($WslDir -eq '~') { $WslDirAbs = $WslHome }
    elseif ($WslDir.StartsWith('~/')) { $WslDirAbs = $WslHome + $WslDir.Substring(1) }
    else { $WslDirAbs = $WslDir }
    $WslDirAbs = $WslDirAbs.TrimEnd('/')
    $WslData = $WslDirAbs + '-data/native'
    $WslBin = $WslDirAbs + '/target/release/dongsi-server'
    $WslHelper = $WslDirAbs + '/scripts/linux-native-server.sh'
    Say ("WSL 클론: $WslDirAbs, Modes=" + ($Modes -join ',') + ', Clients=' + ($Clients -join ',') + ", PortBase=$PortBase, LAN=$LanIp")

    # ---- 코드 옮기기(HEAD) -----------------------------------------------
    $Commit = (& git -C $RepoRoot rev-parse HEAD).Trim()
    $dirty = (& git -C $RepoRoot status --porcelain) -join $LF
    if ($dirty.Trim()) { Warn '작업 트리에 커밋하지 않은 변경이 있습니다. 시험은 커밋한 상태(HEAD)로 합니다' }
    $bundleDir = Join-Path ([System.IO.Path]::GetTempPath()) ('dongsi-e2e-' + $PID)
    New-Item -ItemType Directory -Force -Path $bundleDir | Out-Null
    $bundle = Join-Path $bundleDir 'dongsi-e2e.bundle'
    if (Test-Path -LiteralPath $bundle) { Remove-Item -LiteralPath $bundle -Force }
    $br = Invoke-Proc 'git' @('-C', $RepoRoot, 'bundle', 'create', $bundle, 'HEAD') (Join-Path $RunDir 'bundle.log') 300
    if ($br.ExitCode -ne 0) { Fail ('git bundle 실패:' + $LF + (Tail $br.Text)) }
    $syncBody = @'
set -eu
D=@@DIR@@
mkdir -p "$D"
cd "$D"
[ -d .git ] || git init -q .
git fetch -q @@BUNDLE@@ HEAD
git checkout -q --detach FETCH_HEAD
echo "COMMIT=$(git rev-parse HEAD)"
'@
    $syncBody = $syncBody.Replace('@@DIR@@', (Q $WslDirAbs)).Replace('@@BUNDLE@@', (Q (ConvertTo-WslPath $bundle)))
    $sync = Invoke-Wsl 'sync' $syncBody 600
    if ($sync.ExitCode -ne 0) { Fail ('WSL로 코드를 옮기지 못했습니다:' + $LF + (Tail $sync.Text)) }
    if ($sync.Text -notmatch ('COMMIT=' + $Commit)) { Fail "WSL의 커밋이 다릅니다. 로그: $($sync.Log)" }
    Remove-Item -LiteralPath $bundle -Force -ErrorAction SilentlyContinue
    Say "시험할 커밋: $Commit"

    # ---- 빌드 -------------------------------------------------------------
    if ($SkipBuild) {
        Say '빌드 건너뜀(-SkipBuild)'
    } else {
        if ($HasDocker) {
            Say "빌드: docker build -t $ImageTag (처음에는 몇 분 걸립니다)"
            $r = Invoke-Wsl 'build-docker' ('set -eu' + $LF + 'cd ' + (Q $WslDirAbs) + $LF + 'docker build -t ' + (Q $ImageTag) + ' .') 3600
            if ($r.ExitCode -ne 0) { Fail ("docker build 실패. 로그: $($r.Log)" + $LF + (Tail $r.Text)) }
            Say ('  완료 {0:N0}초' -f $r.Seconds)
        }
        if ($HasNative) {
            Say 'WSL 빌드: cargo build -p dongsi-server-app --release'
            $r = Invoke-Wsl 'build-native' ('set -eu' + $LF + 'cd ' + (Q $WslDirAbs) + $LF + 'cargo build -p dongsi-server-app --release') 3600
            if ($r.ExitCode -ne 0) { Fail ("서버 빌드 실패. 로그: $($r.Log)" + $LF + (Tail $r.Text)) }
            Say ('  완료 {0:N0}초' -f $r.Seconds)
        }
        if ($HasWslClient) {
            Say 'WSL 빌드: cargo test -p dongsi-server --test e2e --no-run'
            $r = Invoke-Wsl 'build-wsl-test' ('set -eu' + $LF + 'cd ' + (Q $WslDirAbs) + $LF + 'cargo test -p dongsi-server --test e2e --no-run') 3600
            if ($r.ExitCode -ne 0) { Fail ("WSL 시험 빌드 실패. 로그: $($r.Log)" + $LF + (Tail $r.Text)) }
            Say ('  완료 {0:N0}초' -f $r.Seconds)
        }
        if ($HasWinClient) {
            Say 'Windows 빌드: cargo test -p dongsi-server --test e2e --no-run'
            $r = Invoke-Proc 'cargo' @('test', '-p', 'dongsi-server', '--test', 'e2e', '--no-run') (Join-Path $RunDir 'build-windows-test.log') 3600 $RepoRoot
            if ($r.ExitCode -ne 0) { Fail ("Windows 시험 빌드 실패. 로그: $($r.Log)" + $LF + (Tail $r.Text)) }
            Say ('  완료 {0:N0}초' -f $r.Seconds)
        }
    }
    if ($HasDocker -and $SkipBuild) {
        $r = Invoke-Wsl 'image-check' ('docker image inspect ' + (Q $ImageTag) + ' >/dev/null 2>&1') 60
        if ($r.ExitCode -ne 0) { Fail "이미지 $ImageTag 가 없습니다(-SkipBuild를 빼고 다시 실행하세요)" }
    }

    # ---- 서버 다루기 ---------------------------------------------------------
    $inviteFn = @'
invite_of() { docker logs "$1" 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -o 'dongsi1:[A-Za-z0-9_-]\+' | tail -n 1; }
count_of() { docker logs "$1" 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -c 'dongsi1:[A-Za-z0-9_-]\+'; }

'@

    # 서버 하나를 새로 띄우고 초대 코드를 돌려준다.
    function Start-ModeServer([string]$mode) {
        $port = $ModePort[$mode]
        $cport = $ModeCPort[$mode]
        $cname = Container-Name $mode
        if ($mode -eq 'native') {
            $body = @'
H=@@HELPER@@
D=@@DATA@@
# 이전 시험이 남긴 같은 데이터 폴더의 서버를 끄고 폴더를 비운다(이 폴더만).
if [ -d "$D" ]; then bash "$H" stop --data "$D" >&2 || true; fi
case "$D" in *-data/native) rm -rf -- "$D" ;; *) echo "데이터 폴더가 예상 밖입니다: $D" >&2; exit 1 ;; esac
bash "$H" start --name @@NAME@@ --port @@PORT@@ --data "$D" --bin @@BIN@@
'@
            $body = $body.Replace('@@HELPER@@', (Q $WslHelper)).Replace('@@DATA@@', (Q $WslData)).Replace('@@NAME@@', (Q ('e2e-' + $mode))).Replace('@@PORT@@', "$port").Replace('@@BIN@@', (Q $WslBin))
        } else {
            $opts = ''
            switch ($mode) {
                'bridge' { $opts = "-p ${port}:${port}/tcp -p ${port}:${port}/udp -e DONGSI_PORT=$port" }
                'host'   { $opts = "--network host -e DONGSI_PORT=$port" }
                'remap'  { $opts = "-p ${port}:${cport}/tcp -p ${port}:${cport}/udp -e DONGSI_PORT=$cport -e DONGSI_PUBLIC_PORT=$port" }
            }
            $body = $inviteFn + @'
C=@@C@@
docker rm -f "$C" >/dev/null 2>&1 || true
docker volume rm "$C" >/dev/null 2>&1 || true
docker run -d --name "$C" @@OPTS@@ -v "$C":/data -e DONGSI_NAME=@@NAME@@ @@IMAGE@@ >/dev/null || exit 1
for i in $(seq 1 300); do
  code="$(invite_of "$C")"
  if [ -n "$code" ]; then echo "$code"; exit 0; fi
  sleep 0.1
done
echo "30초 안에 초대 코드가 나오지 않았습니다. 컨테이너 로그:" >&2
docker logs "$C" 2>&1 | tail -n 20 >&2
exit 1
'@
            $body = $body.Replace('@@C@@', (Q $cname)).Replace('@@OPTS@@', $opts).Replace('@@NAME@@', (Q ('e2e-' + $mode))).Replace('@@IMAGE@@', (Q $ImageTag))
        }
        $r = Invoke-Wsl ('start-' + $mode) $body 300
        $code = Last-Invite $r.Text
        if ($r.ExitCode -ne 0 -or -not $code) { Fail ("서버($mode)를 시작하지 못했습니다. 로그: $($r.Log)" + $LF + (Tail $r.Text)) }
        [void]$Started.Add($mode)
        return $code
    }

    # 서버를 재시작하고 다시 접속을 받을 때까지 기다린다(비밀번호 실패 기록을 지우려고).
    function Restart-ModeServer([string]$mode) {
        $cname = Container-Name $mode
        if ($mode -eq 'native') {
            $body = 'bash ' + (Q $WslHelper) + ' restart --data ' + (Q $WslData) + ' >&2'
        } else {
            $body = $inviteFn + @'
C=@@C@@
before="$(count_of "$C")"
docker restart -t 10 "$C" >/dev/null || exit 1
for i in $(seq 1 300); do
  n="$(count_of "$C")"
  if [ "$n" -gt "$before" ]; then exit 0; fi
  sleep 0.1
done
echo "재시작한 서버가 30초 안에 시작하지 않았습니다" >&2
exit 1
'@
            $body = $body.Replace('@@C@@', (Q $cname))
        }
        $r = Invoke-Wsl ('restart-' + $mode) $body 120
        if ($r.ExitCode -ne 0) { Fail ("서버($mode) 재시작 실패. 로그: $($r.Log)" + $LF + (Tail $r.Text)) }
        Start-Sleep -Milliseconds 500
    }

    # 시험이 서버를 재시작할 때 쓰는 명령(WSL 안에서 실행할 문자열).
    function Get-RestartCmd([string]$mode) {
        $stop = ''
        if ($env:DONGSI_STOP_SIGNAL) { $stop = 'env DONGSI_STOP_SIGNAL=' + $env:DONGSI_STOP_SIGNAL + ' ' }
        if ($mode -eq 'native') {
            return $stop + 'bash ' + $WslHelper + ' restart --data ' + $WslData
        }
        return 'docker restart -t 10 ' + (Container-Name $mode)
    }

    # 시험이 끝난 서버의 로그를 결과 폴더에 모은다.
    function Save-ServerLog([string]$mode) {
        if ($mode -eq 'native') { $body = 'cat ' + (Q ($WslData + '/dongsi-server.log')) + ' 2>&1' }
        else { $body = 'docker logs ' + (Q (Container-Name $mode)) + ' 2>&1' }
        [void](Invoke-Wsl ('serverlog-' + $mode) $body 60)
    }

    # ---- 시험 한 번 ------------------------------------------------------------
    function Parse-Run([string]$text) {
        $o = @{ Passed = $null; Failed = $null; Ignored = $null; Skips = @(); FailedTests = @(); Av = ''; AvMax = '' }
        $m = [regex]::Matches($text, 'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored')
        if ($m.Count -gt 0) {
            $l = $m[$m.Count - 1]
            $o.Passed = [int]$l.Groups[1].Value; $o.Failed = [int]$l.Groups[2].Value; $o.Ignored = [int]$l.Groups[3].Value
        }
        $skips = @()
        foreach ($s in [regex]::Matches($text, '\[e2e\] 건너뜀: ([A-Za-z0-9_]+)')) { $skips += $s.Groups[1].Value }
        $o.Skips = @($skips | Select-Object -Unique)
        $ft = @()
        foreach ($f in [regex]::Matches($text, '(?m)^test (\S+) \.\.\. FAILED')) { $ft += $f.Groups[1].Value }
        $o.FailedTests = @($ft | Select-Object -Unique)
        $av = [regex]::Match($text, '\[e2e\] A/V 차이: [^\r\n]+')
        if ($av.Success) {
            $o.Av = $av.Value.Substring(6)
            $mx = [regex]::Match($av.Value, '최대 (-?[\d.]+)ms')
            if ($mx.Success) { $o.AvMax = $mx.Groups[1].Value + 'ms' }
        }
        return $o
    }

    function Run-One([string]$mode, [string]$client, [string]$addr, [bool]$destructive) {
        $kind = '일반'
        $kindTag = 'normal'
        if ($destructive) { $kind = '파괴적'; $kindTag = 'destructive' }
        $invite = $script:Invites[$mode]
        # 감지 응답은 서버가 실제로 듣는 포트를 알린다. 포트를 다르게 매핑한 remap에서는 컨테이너 포트다.
        $discoveryPort = $null
        if ($ModeCPort[$mode] -ne $ModePort[$mode]) { $discoveryPort = $ModeCPort[$mode] }
        $tag = ('{0}-{1}-{2}-{3}' -f $mode, $client, ($addr -replace '[^0-9A-Za-z]', '_'), $kindTag)
        $log = Join-Path $RunDir ('run-' + $tag + '.log')
        Say "실행: $mode / $client / $addr / $kind"
        $filters = @()
        if ($destructive) { $filters = @('rate_limit', 'server_restart') }

        if ($client -eq 'windows') {
            $vars = @{ DONGSI_E2E_SERVER = $addr; DONGSI_E2E_INVITE = $invite; DONGSI_E2E_DESTRUCTIVE = $null; DONGSI_E2E_RESTART_CMD = $null; DONGSI_E2E_DISCOVERY_PORT = $null }
            if ($discoveryPort) { $vars['DONGSI_E2E_DISCOVERY_PORT'] = "$discoveryPort" }
            if ($destructive) {
                $vars['DONGSI_E2E_DESTRUCTIVE'] = '1'
                $vars['DONGSI_E2E_RESTART_CMD'] = 'wsl.exe -e ' + (Get-RestartCmd $mode)
            }
            $cargoArgs = @('test', '-p', 'dongsi-server', '--test', 'e2e', '--', '--nocapture') + $filters
            $r = With-Env $vars { Invoke-Proc 'cargo' $cargoArgs $log 900 $RepoRoot }
        } else {
            $body = 'set -u' + $LF + 'cd ' + (Q $WslDirAbs) + $LF +
                'export DONGSI_E2E_SERVER=' + (Q $addr) + $LF +
                'export DONGSI_E2E_INVITE=' + (Q $invite) + $LF
            if ($discoveryPort) { $body += 'export DONGSI_E2E_DISCOVERY_PORT=' + $discoveryPort + $LF }
            if ($destructive) {
                $body += 'export DONGSI_E2E_DESTRUCTIVE=1' + $LF + 'export DONGSI_E2E_RESTART_CMD=' + (Q (Get-RestartCmd $mode)) + $LF
            }
            $body += 'cargo test -p dongsi-server --test e2e -- --nocapture ' + ($filters -join ' ') + $LF
            $r = Invoke-Wsl ('run-' + $tag) $body 900
            $log = $r.Log
        }

        $p = Parse-Run $r.Text
        $note = ''
        $ok = ($r.ExitCode -eq 0) -and (-not $r.TimedOut) -and ($null -ne $p.Passed) -and ($p.Failed -eq 0)
        if ($r.TimedOut) { $note = '시간 초과' }
        elseif ($null -eq $p.Passed) { $note = '시험 결과 줄이 없음(빌드·실행 실패?)' }
        elseif ($p.Failed -gt 0) { $note = '실패: ' + ($p.FailedTests -join ', ') }
        elseif ($r.ExitCode -ne 0) { $note = "종료 코드 $($r.ExitCode)" }
        if ($ok -and $destructive) {
            # 파괴적 시험이 건너뛰어졌으면 아무것도 증명하지 못한 것이다.
            $skipped = @($p.Skips | Where-Object { $_ -eq 'rate_limit' -or $_ -eq 'server_restart' })
            if ($skipped.Count -gt 0 -or $p.Passed -lt 2) { $ok = $false; $note = '파괴적 시험이 건너뛰어짐: ' + ($skipped -join ', ') }
        }
        $status = 'FAIL'
        if ($ok) { $status = 'PASS' }
        $row = [pscustomobject]@{
            Mode = $mode; Client = $client; Addr = $addr; Kind = $kind; Status = $status
            Passed = $p.Passed; Failed = $p.Failed; Ignored = $p.Ignored
            Skips = ($p.Skips -join ','); AvMax = $p.AvMax; Av = $p.Av
            Secs = [math]::Round($r.Seconds, 1); Log = $log; Note = $note
        }
        [void]$Results.Add($row)
        $color = 'Green'
        if (-not $ok) { $color = 'Red' }
        Write-Host ('       {0}: 통과 {1} 실패 {2} 무시 {3}, {4:N1}초 {5}' -f $status, $p.Passed, $p.Failed, $p.Ignored, $r.Seconds, $note) -ForegroundColor $color
        if (-not $ok) { Write-Host (Tail $r.Text 12) -ForegroundColor DarkGray }
    }

    function Add-StartFailure([string]$mode, [string]$msg) {
        [void]$Results.Add([pscustomobject]@{
            Mode = $mode; Client = '-'; Addr = '-'; Kind = '서버'; Status = 'FAIL'
            Passed = $null; Failed = $null; Ignored = $null; Skips = ''; AvMax = ''; Av = ''
            Secs = 0; Log = ''; Note = $msg })
    }

    # ---- 모드별 시험 -------------------------------------------------------------
    foreach ($mode in $Modes) {
        $port = $ModePort[$mode]
        Say "===== 모드 $mode (포트 $port) ====="
        try {
            $script:Invites[$mode] = Start-ModeServer $mode
        } catch {
            Warn ('서버 시작 실패: ' + $_.Exception.Message)
            Add-StartFailure $mode (First-Line $_.Exception.Message)
            $exitCode = 1
            continue
        }
        Say ('서버 시작됨. 초대 코드 ' + $script:Invites[$mode].Substring(0, 24) + '…')

        # 클라이언트별 주소
        $plan = @()
        foreach ($c in $Clients) {
            if ($c -eq 'windows') {
                $plan += ,@('windows', "127.0.0.1:$port")
                if ($LanIp) { $plan += ,@('windows', "${LanIp}:$port") }
            } else {
                $plan += ,@('wsl', "127.0.0.1:$port")
            }
        }
        $first = $true
        try {
            foreach ($item in $plan) {
                if (-not $first) { Restart-ModeServer $mode }
                $first = $false
                Run-One $mode $item[0] $item[1] $false
            }
            if (-not $SkipDestructive) {
                foreach ($c in $Clients) {
                    Restart-ModeServer $mode
                    Run-One $mode $c "127.0.0.1:$port" $true
                }
            }
        } catch {
            Warn ("모드 $mode 진행 중 오류: " + $_.Exception.Message)
            Add-StartFailure $mode (First-Line $_.Exception.Message)
        }
        try { Save-ServerLog $mode } catch { }
    }
}
catch {
    Write-Host ('[e2e] 오류: ' + $_.Exception.Message) -ForegroundColor Red
    $exitCode = 1
}
finally {
    # ---- 정리: 우리가 띄운 서버만 끈다 ----------------------------------------------
    if ($KeepServers) {
        if ($Started.Count -gt 0) { Say ('서버를 남겨 둡니다(-KeepServers): ' + (($Started | Select-Object -Unique) -join ', ')) }
    } elseif ($Started.Count -gt 0) {
        Say '서버 정리'
        foreach ($mode in ($Started | Select-Object -Unique)) {
            try {
                if ($mode -eq 'native') {
                    $body = 'bash ' + (Q $WslHelper) + ' stop --data ' + (Q $WslData)
                } else {
                    $c = Q (Container-Name $mode)
                    $body = "docker rm -f $c >/dev/null 2>&1" + $LF + "docker volume rm $c >/dev/null 2>&1" + $LF + 'true'
                }
                [void](Invoke-Wsl ('cleanup-' + $mode) $body 120)
            } catch { Warn ("정리 실패($mode): " + $_.Exception.Message) }
        }
    }
}

# ---------------------------------------------------------------------------
# 결과

$TotalWatch.Stop()
$failCount = @($Results | Where-Object { $_.Status -ne 'PASS' }).Count
if ($failCount -gt 0) { $exitCode = 1 }
if ($Results.Count -eq 0) { $exitCode = 1 }

$sb = New-Object System.Text.StringBuilder
[void]$sb.AppendLine('# 리눅스 서버 E2E 시험 결과')
[void]$sb.AppendLine('')
[void]$sb.AppendLine("- 시각: $Stamp")
[void]$sb.AppendLine("- 시험한 커밋: $Commit")
[void]$sb.AppendLine('- 모드: ' + ($Modes -join ', ') + ' / 클라이언트: ' + ($Clients -join ', ') + " / PortBase $PortBase / LAN $LanIp")
[void]$sb.AppendLine("- WSL 클론: $WslDirAbs / 이미지: $ImageTag")
[void]$sb.AppendLine(('- 총 걸린 시간: {0:N0}초({1:N1}분)' -f $TotalWatch.Elapsed.TotalSeconds, $TotalWatch.Elapsed.TotalMinutes))
$verdict = '모두 통과'
if ($exitCode -ne 0) { $verdict = '실패 있음' }
[void]$sb.AppendLine("- 결론: $verdict")
[void]$sb.AppendLine('')
[void]$sb.AppendLine('| 모드 | 클라이언트 | 주소 | 구분 | 결과 | 통과/실패/무시 | 건너뜀 | A/V 최대 | 시간(초) | 비고 |')
[void]$sb.AppendLine('|---|---|---|---|---|---|---|---|---|---|')
foreach ($r in $Results) {
    $counts = '-'
    if ($null -ne $r.Passed) { $counts = "$($r.Passed)/$($r.Failed)/$($r.Ignored)" }
    [void]$sb.AppendLine(('| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} | {9} |' -f $r.Mode, $r.Client, $r.Addr, $r.Kind, $r.Status, $counts, $r.Skips, $r.AvMax, $r.Secs, $r.Note))
}
[void]$sb.AppendLine('')
[void]$sb.AppendLine('로그: 같은 폴더의 run-*.log(실행마다), *-serverlog-*.log(서버 로그), build-*.log')
$summaryPath = Join-Path $RunDir 'summary.md'
[System.IO.File]::WriteAllText($summaryPath, $sb.ToString(), $Utf8NoBom)

Write-Host ''
Write-Host '=============== 결과 ==============='
$Results | Select-Object Mode, Client, Addr, Kind, Status, Passed, Failed, Ignored, Skips, AvMax, Secs |
    Format-Table -AutoSize | Out-String -Width 220 | Write-Host
foreach ($r in $Results) {
    if ($r.Status -ne 'PASS') { Write-Host ('실패: {0}/{1}/{2}/{3}: {4} ({5})' -f $r.Mode, $r.Client, $r.Addr, $r.Kind, $r.Note, $r.Log) -ForegroundColor Red }
}
Say ('커밋 {0}, 총 {1:N0}초, 요약: {2}' -f $Commit, $TotalWatch.Elapsed.TotalSeconds, $summaryPath)
if ($exitCode -eq 0) { Write-Host '[e2e] 모두 통과' -ForegroundColor Green } else { Write-Host '[e2e] 실패가 있습니다' -ForegroundColor Red }
exit $exitCode
