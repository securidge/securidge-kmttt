# Securidge KMttt 보안 E2E 점검(Windows). 실제 서버(named pipe)·ConPTY·PowerShell pane으로 확인한다.
# 사용: pwsh -File ci/e2e-windows.ps1 [kmttt.exe 경로]
# 로컬 실행은 임시 세션·폴더로 격리한다. GitHub Actions에서는 기존 default 세션을 쓴다.
param([string]$Kmttt = "target\debug\kmttt.exe", [string]$Session, [switch]$Force)

function Assert-E2EExternalTerminal([string]$pane) {
    if ($pane) {
        throw 'KMttt 밖 터미널에서 실행하세요. Run this E2E check from a terminal outside KMttt.'
    }
}

# 환경 변경·실행 파일 조회·임시 폴더 생성 전에 거부한다. 호출자 식별 값은 지우지 않는다.
Assert-E2EExternalTerminal $env:KMTTT_PANE

# PowerShell의 출력 캡처 대신 cmd가 리다이렉트 파일 핸들을 직접 열어 자식에게 물려준다.
# ProcessStartInfo로 명령줄을 넘겨 PowerShell 5.1의 native 인자 따옴표 손실을 피한다.
function Invoke-E2ERedirectedList([string]$exe, [string]$sessionName, [string]$outputFile) {
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $env:ComSpec
    $start.Arguments = '/d /s /c ""' + $exe + '" --session ' + $sessionName + ' workspace list > "' + $outputFile + '""'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($start)
    try {
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "Redirected workspace list failed: exit $($process.ExitCode)" }
    } finally { $process.Dispose() }
}

function New-E2EContext([string]$sessionName, [bool]$isCI, [bool]$forceDefault) {
    if (-not $sessionName) {
        if ($isCI) { $sessionName = 'default' }
        else { $sessionName = 'e2e-' + [guid]::NewGuid().ToString('N') }
    }
    if ($sessionName -cnotmatch '\A[A-Za-z0-9_-]{1,64}\z') { throw 'Invalid E2E session name' }
    if (-not $isCI -and $sessionName -eq 'default' -and -not $forceDefault) {
        throw 'Local E2E refuses session default. Use a temporary session, or explicitly pass -Force.'
    }
    $root = $null
    if (-not $isCI) {
        $root = Join-Path ([IO.Path]::GetTempPath()) ('kmttt-e2e-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $root -ErrorAction Stop | Out-Null
    }
    @{ Session = $sessionName; Root = $root }
}

function Remove-E2ETempRoot([string]$root) {
    if (-not $root) { return }
    $full = [IO.Path]::GetFullPath($root)
    $tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path $full -Leaf) -cnotmatch '^kmttt-e2e-[a-f0-9]{32}$') {
        throw "Refusing to remove unexpected E2E directory: $full"
    }
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        try {
            if (Test-Path -LiteralPath $full) { Remove-Item -LiteralPath $full -Recurse -Force -ErrorAction Stop }
            return
        } catch {
            if ($attempt -eq 39) { throw }
            Start-Sleep -Milliseconds 250
        }
    }
}

# WMI는 환경을 상속하지 않는다. 로컬 점검에서는 인코딩한 PowerShell로 격리 경로를 명시한다.
function New-E2EWmiCommand([string]$command, [bool]$isCI) {
    if ($isCI) { return $command }
    $setup = foreach ($key in @('KMTTT_CONFIG_DIR', 'KMTTT_DATA_DIR', 'KMTTT_SOCKET_DIR', 'KMTTT_SESSION_NAME')) {
        '$env:' + $key + " = '" + ([Environment]::GetEnvironmentVariable($key)).Replace("'", "''") + "';"
    }
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(($setup -join "`n") + "`n" + $command))
    "powershell.exe -NoProfile -NonInteractive -EncodedCommand $encoded"
}

$ErrorActionPreference = "Continue"
# Windows PowerShell 5.1 호환: kmttt 출력(UTF-8)을 제대로 읽고, 외부 명령 인자에 큰따옴표를 넣지 않는다
# (5.1은 인자 속 큰따옴표를 깨뜨리고 빈 문자열 인자를 버린다).
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$K = (Resolve-Path -LiteralPath $Kmttt -ErrorAction Stop).Path
$isCI = $env:GITHUB_ACTIONS -eq 'true'
$savedEnv = @{}
foreach ($key in @('KMTTT_CONFIG_DIR', 'KMTTT_DATA_DIR', 'KMTTT_SOCKET_DIR', 'KMTTT_SESSION_NAME', 'KMTTT_PANE')) {
    $savedEnv[$key] = [Environment]::GetEnvironmentVariable($key)
}
$context = New-E2EContext $Session $isCI $Force
$Session = $context.Session
$console = $null
$serverAttempted = $false
try {
if ($context.Root) {
    $env:KMTTT_CONFIG_DIR = Join-Path $context.Root 'config'
    $env:KMTTT_DATA_DIR = Join-Path $context.Root 'data'
    $env:KMTTT_SOCKET_DIR = Join-Path $context.Root 'socket'
    Write-Host "== isolated local E2E: session=$Session root=$($context.Root)"
}
# 세션 이름은 환경 변수로 자식들(Start-Process 포함)에게 넘기고, 환경을 못 물려받는 WMI 명령줄에는 직접 적는다.
$env:KMTTT_SESSION_NAME = $Session
$script:Pass = 0
$script:Fail = 0

function Check([string]$name, [scriptblock]$cond) {
    $ok = $false
    try { $ok = [bool](& $cond) } catch { $ok = $false }
    if ($ok) { $script:Pass++; Write-Host "  ok   $name" }
    else { $script:Fail++; Write-Host "  FAIL $name" }
}
function WaitFor([int]$seconds, [scriptblock]$cond) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        try { if (& $cond) { return $true } } catch {}
        Start-Sleep -Milliseconds 250
    }
    return $false
}
# 5.1의 ConvertFrom-Json은 배열을 객체 하나로 내보내므로 원소별로 풀어 준다.
function Kj { $j = ((& $K @args) | Out-String) | ConvertFrom-Json; $j | ForEach-Object { $_ } }
function Approvals { $a = Kj approval list; if ($null -eq $a) { @() } else { @($a) } }
function PaneText([string]$pane, [int]$lines = 30) { (& $K pane read $pane --lines $lines) | Out-String }
function Chain([int]$procId) {
    $names = @()
    for ($i = 0; $i -lt 12 -and $procId; $i++) {
        $p = Get-CimInstance Win32_Process -Filter "ProcessId=$procId" -ErrorAction SilentlyContinue
        if (-not $p) { break }
        $names += $p.Name
        $procId = $p.ParentProcessId
    }
    $names -join " <- "
}

Write-Host "== environment"
Write-Host "  caller lineage of this script: $(Chain $PID)"
if ($isCI) {
    # CI 러너는 Windows 서비스 밑에서 돈다. 러너만 믿을 실행자로 등록한다(WMI 우회는 계속 막혀야 함).
    $policy = ((& $K policy path) | Out-String).Trim()
    New-Item -ItemType Directory -Force (Split-Path $policy) | Out-Null
    Set-Content -Path $policy -Value "[identify]`ntrusted_launchers = [`"Runner.Listener`"]" -Encoding utf8NoBOM
    Write-Host "  wrote $policy (trusted_launchers = Runner.Listener)"
}

Write-Host "== layout (named pipe server, ConPTY panes)"
$serverAttempted = $true
& $K workspace create --label e2e | Out-Null
$A = @(Kj pane list)[-1].id
$B = (Kj pane split $A --direction right --no-focus).pane
Check "two panes ($A, $B)" { $A -and $B }
Start-Sleep -Seconds 3
& $K pane run $B "Write-Output ('PANE' + '-OK')" | Out-Null
Check "PowerShell runs inside a ConPTY pane" { WaitFor 20 { (PaneText $B) -match "PANE-OK" } }
if ($script:Fail -gt 0) {
    Write-Host "---- diagnostics: pane list"
    (& $K pane list) | Out-String | Write-Host
    Write-Host "---- pane read (screen)"
    (& $K pane read $B --source screen --lines 20) | Out-String | Write-Host
    $log = if ($context.Root) { Join-Path $env:KMTTT_DATA_DIR 'kmttt-server.log' } else { Join-Path $env:LOCALAPPDATA "Securidge\kmttt\kmttt-server.log" }
    Write-Host "---- server log ($log)"
    Get-Content $log -Tail 40 -ErrorAction SilentlyContinue | Write-Host
}

Write-Host "== F1 agent-to-agent approval"
& $K pane run $A "& '$K' pane run $B 'Write-Output E2E-ALLOWED'; Write-Output ('RC1=' + `$LASTEXITCODE)" | Out-Null
# 5.1은 단일 객체의 .Count를 지원하지 않으므로 @()로 감싼다.
Check "pane -> pane run is queued" { WaitFor 20 { @(Approvals).Count -ge 1 } }
$first = @(Approvals)[0]
Check "queued request names the calling pane ($A)" { $first.caller -eq $A }
& $K approval allow $first.id | Out-Null
Check "allowed run reaches the target" { WaitFor 20 { (PaneText $B) -match "(?m)^E2E-ALLOWED" } }
& $K pane run $A "& '$K' pane run $B 'Write-Output E2E-DENIED'; Write-Output ('RC2=' + `$LASTEXITCODE)" | Out-Null
WaitFor 20 { @(Approvals).Count -ge 1 } | Out-Null
& $K approval deny @(Approvals)[0].id | Out-Null
Check "denied run exits 3" { WaitFor 20 { (PaneText $A) -match "RC2=3" } }

Write-Host "== F1 escape attempts"
# 1) 부모와 분리된 프로세스(Start-Process): pane Job 안에 남아야 한다.
# Start-Process는 인자를 따옴표 없이 이어 붙이므로 명령줄 한 줄로 넘긴다.
# 공백 없는 명령([console]::WriteLine)이라 따옴표 없이 넘어간다.
& $K pane run $A "Start-Process -WindowStyle Hidden -FilePath '$K' -ArgumentList 'pane run $B [console]::WriteLine(''ESC1'')'" | Out-Null
Check "Start-Process child is still identified as the pane (Job Object)" {
    WaitFor 20 { @(Approvals | Where-Object { $_.caller -eq $A }).Count -ge 1 }
}
# 2) WMI로 만든 프로세스: pane Job 밖, 부모는 WmiPrvSE → unknown 에이전트여야 한다.
# WMI 명령줄: kmttt 경로에 공백이 없다고 보고 따옴표 없이 쓴다(이 점검용 폴더).
$wmiCmd = "$K --session $Session pane run $B [console]::WriteLine(''ESC2'')"
if (-not $isCI) {
    $wmiCmd = New-E2EWmiCommand ("& '" + $K.Replace("'", "''") + "' --session $Session pane run $B 'Write-Output ESC2'") $isCI
}
& $K pane run $A "Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = '$wmiCmd' } | Out-Null" | Out-Null
Check "WMI-created process is treated as an agent, not a person" {
    WaitFor 25 { @(Approvals | Where-Object { $_.caller -eq "unknown" }).Count -ge 1 }
}
# 3) WMI로 스스로 승인 시도 → 거부되어야 한다.
$id = @(Approvals)[0].id
$selfLog = Join-Path $(if ($context.Root) { $context.Root } else { Split-Path $K }) "kmttt-selfapprove.txt"
Remove-Item $selfLog -ErrorAction SilentlyContinue
$approveCmd = "cmd /c $K --session $Session approval allow $id > $selfLog 2>&1"
if (-not $isCI) {
    $approveCmd = New-E2EWmiCommand ("& '" + $K.Replace("'", "''") + "' --session $Session approval allow $id *> '" + $selfLog.Replace("'", "''") + "'") $isCI
}
& $K pane run $A "Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = '$approveCmd' } | Out-Null" | Out-Null
Start-Sleep -Seconds 6
Check "WMI-created process cannot approve requests" { (Get-Content $selfLog -Raw -ErrorAction SilentlyContinue) -match "denied|person|not allowed" }
# 주의: PowerShell 변수 이름은 대소문자를 가리지 않는다($a와 $A는 같다).
foreach ($pending in Approvals) { & $K approval deny $pending.id | Out-Null }
Start-Sleep -Seconds 2
Check "escaped commands never ran" { -not ((PaneText $B 60) -match "(?m)^ESC[12]\s*$") }

Write-Host "== F3 redaction"
& $K pane run $A "`$env:E2E_API_TOKEN = 'sk-e2eSECRETvalue1234567890abcd'; Write-Output `$env:E2E_API_TOKEN" | Out-Null
Start-Sleep -Seconds 3
Check "pane read never shows the secret" { -not ((PaneText $A) -match "sk-e2eSECRET") }
Check "secret was replaced by a marker" { (PaneText $A) -match "REDACTED" }

Write-Host "== F5 hook"
$env:KMTTT_PANE = $A
'{"tool_name":"Bash","tool_input":{"command":"rm -rf /"}}' | & $K hook claude PreToolUse 2>$null | Out-Null
$rcBad = $LASTEXITCODE
'{"tool_name":"Bash","tool_input":{"command":"cargo test"}}' | & $K hook claude PreToolUse 2>$null | Out-Null
$rcOk = $LASTEXITCODE
$env:KMTTT_PANE = $savedEnv['KMTTT_PANE']
Check "dangerous tool call blocked (exit 2)" { $rcBad -eq 2 }
Check "safe tool call allowed (exit 0)" { $rcOk -eq 0 }

Write-Host "== F4 restricted profile (Job Object limits)"
$proj = Join-Path $(if ($context.Root) { $context.Root } else { $env:TEMP }) "kmttt-e2e-proj"
New-Item -ItemType Directory -Force $proj | Out-Null
& $K workspace create --label sbx --profile restricted --cwd $proj --no-focus | Out-Null
Start-Sleep -Seconds 3
$R = @(Kj pane list)[-1].id
& $K pane run $R "Write-Output ('SBX' + '-OK')" | Out-Null
Check "restricted pane starts and runs commands" { WaitFor 20 { (PaneText $R) -match "SBX-OK" } }
Check "restricted strength is reported as low on Windows" { ((& $K pane list) | Out-String) -match '"low"' }

Write-Host "== F2 ledger"
Check "ledger verifies" { ((& $K audit verify) | Out-String) -match '"ok": true' }
$ledger = ((& $K audit path) | Out-String).Trim()
Check "ledger never contains the secret" { -not (Select-String -Path (Join-Path $ledger "*.jsonl") -Pattern "sk-e2eSECRET" -Quiet) }
Check "approval decisions record who decided" { Select-String -Path (Join-Path $ledger "*.jsonl") -Pattern "decided_by" -Quiet }

Write-Host "== web console"
$consoleLog = Join-Path $(if ($context.Root) { $context.Root } else { $env:TEMP }) "kmttt-console.txt"
$consoleErr = Join-Path $(if ($context.Root) { $context.Root } else { $env:TEMP }) "kmttt-console-err.txt"
$console = Start-Process -FilePath $K -ArgumentList "--session $Session console --no-open" -RedirectStandardOutput $consoleLog -RedirectStandardError $consoleErr -PassThru -WindowStyle Hidden
function ConsoleText { ((Get-Content $consoleLog -Raw -ErrorAction SilentlyContinue) + (Get-Content $consoleErr -Raw -ErrorAction SilentlyContinue)) }
WaitFor 15 { (ConsoleText) -match "http://" } | Out-Null
$url = ([regex]::Match([string](ConsoleText), "http://\S+")).Value
$base = $url -replace "/\?t=.*$", ""
$tok = $url -replace "^.*t=", ""
function Code($uri, $headers = @{}, $method = "GET") {
    try { (Invoke-WebRequest -Uri $uri -Headers $headers -Method $method -UseBasicParsing).StatusCode }
    catch { [int]$_.Exception.Response.StatusCode }
}
Check "API needs the token" { (Code "$base/api/snapshot") -eq 401 }
Check "API works with the token header" { (Code "$base/api/snapshot" @{ "X-KMttt-Token" = $tok }) -eq 200 }
Check "actions are off by default" { (Code "$base/api/approval/1/allow" @{ "X-KMttt-Token" = $tok } "POST") -eq 403 }
Check "favicon served" { (Code "$base/favicon.svg") -eq 200 }
Stop-Process -Id $console.Id -ErrorAction SilentlyContinue

Write-Host "== persistence"
& $K server stop | Out-Null
Start-Sleep -Seconds 2
# 서버를 띄운 CLI의 출력을 파일로 돌렸을 때, 오래 사는 서버가 그 파일을 쥐고 있으면 안 된다.
$leak = Join-Path $(if ($context.Root) { $context.Root } else { Split-Path $K }) "kmttt-leak-check.txt"
if ($isCI) { cmd /c "$K --session $Session workspace list > $leak" }
else { Invoke-E2ERedirectedList $K $Session $leak }
Start-Sleep -Seconds 1
Check "auto-started server does not hold the caller's redirect file" { Remove-Item $leak -ErrorAction Stop; $true }
Check "layout restored after restart" { WaitFor 20 { ((& $K pane list) | Out-String) -match [regex]::Escape("""$B""") } }
& $K server stop | Out-Null

Write-Host ""
Write-Host "passed: $script:Pass  failed: $script:Fail"
if ($script:Fail -ne 0) { exit 1 }
} finally {
    try {
        $env:KMTTT_PANE = $savedEnv['KMTTT_PANE']
        if ($console -and -not $console.HasExited) { Stop-Process -InputObject $console -ErrorAction SilentlyContinue }
        if ($context.Root) {
            if ($serverAttempted) { & $K --session $Session server stop 2>$null | Out-Null }
            Remove-E2ETempRoot $context.Root
        }
    } finally {
        foreach ($key in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($key, $savedEnv[$key]) }
    }
}
