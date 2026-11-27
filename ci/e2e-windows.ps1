# Securidge KMttt 보안 E2E 점검(Windows). 실제 서버(named pipe)·ConPTY·PowerShell pane으로 확인한다.
# 사용: pwsh -File ci/e2e-windows.ps1 [kmttt.exe 경로]
# WMI로 만든 프로세스는 이 스크립트의 환경 변수를 물려받지 않으므로, 격리 폴더 대신 기본 세션을 쓴다
# (깨끗한 CI 러너나 시험용 계정에서 돌린다).
param([string]$Kmttt = "target\debug\kmttt.exe", [string]$Session = "default")
$ErrorActionPreference = "Continue"
# Windows PowerShell 5.1 호환: kmttt 출력(UTF-8)을 제대로 읽고, 외부 명령 인자에 큰따옴표를 넣지 않는다
# (5.1은 인자 속 큰따옴표를 깨뜨리고 빈 문자열 인자를 버린다).
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$K = (Resolve-Path $Kmttt).Path
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
if ($env:GITHUB_ACTIONS -eq "true") {
    # CI 러너는 Windows 서비스 밑에서 돈다. 러너만 믿을 실행자로 등록한다(WMI 우회는 계속 막혀야 함).
    $policy = ((& $K policy path) | Out-String).Trim()
    New-Item -ItemType Directory -Force (Split-Path $policy) | Out-Null
    Set-Content -Path $policy -Value "[identify]`ntrusted_launchers = [`"Runner.Listener`"]" -Encoding utf8NoBOM
    Write-Host "  wrote $policy (trusted_launchers = Runner.Listener)"
}

Write-Host "== layout (named pipe server, ConPTY panes)"
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
    $log = Join-Path $env:LOCALAPPDATA "Securidge\kmttt\kmttt-server.log"
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
& $K pane run $A "Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = '$wmiCmd' } | Out-Null" | Out-Null
Check "WMI-created process is treated as an agent, not a person" {
    WaitFor 25 { @(Approvals | Where-Object { $_.caller -eq "unknown" }).Count -ge 1 }
}
# 3) WMI로 스스로 승인 시도 → 거부되어야 한다.
$id = @(Approvals)[0].id
$selfLog = Join-Path (Split-Path $K) "kmttt-selfapprove.txt"
Remove-Item $selfLog -ErrorAction SilentlyContinue
$approveCmd = "cmd /c $K --session $Session approval allow $id > $selfLog 2>&1"
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
Remove-Item Env:KMTTT_PANE
Check "dangerous tool call blocked (exit 2)" { $rcBad -eq 2 }
Check "safe tool call allowed (exit 0)" { $rcOk -eq 0 }

Write-Host "== F4 restricted profile (Job Object limits)"
$proj = Join-Path $env:TEMP "kmttt-e2e-proj"
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
$consoleLog = Join-Path $env:TEMP "kmttt-console.txt"
$consoleErr = Join-Path $env:TEMP "kmttt-console-err.txt"
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
$leak = Join-Path (Split-Path $K) "kmttt-leak-check.txt"
cmd /c "$K --session $Session workspace list > $leak"
Start-Sleep -Seconds 1
Check "auto-started server does not hold the caller's redirect file" { Remove-Item $leak -ErrorAction Stop; $true }
Check "layout restored after restart" { WaitFor 20 { ((& $K pane list) | Out-String) -match [regex]::Escape("""$B""") } }
& $K server stop | Out-Null

Write-Host ""
Write-Host "passed: $script:Pass  failed: $script:Fail"
if ($script:Fail -ne 0) { exit 1 }
