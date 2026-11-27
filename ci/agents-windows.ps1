# 실제 에이전트 CLI로 상태 감지를 확인한다(사람이 자기 컴퓨터에서 돌리는 점검, CI용 아님).
# 설치된 에이전트(claude, codex, gemini, copilot)마다 pane에서 띄우고, 짧은 질문을 보낸 뒤
# 상태 변화·근거·화면을 기록한다. 실제 계정으로 아주 짧은 요청을 1번씩 보낸다.
# 사용: pwsh -File agents-windows.ps1 -Kmttt .\kmttt.exe -Work D:\path\sandbox -Out D:\path\results
param(
    [string]$Kmttt = ".\kmttt.exe",
    [string]$Work = ".\sandbox",
    [string]$Out = ".\results",
    [string]$Session = "agents-check",
    # 일부만 다시 볼 때: -Only claude,codex
    [string[]]$Only = @()
)
$ErrorActionPreference = "Continue"
# Windows PowerShell 5.1 호환(e2e-windows.ps1 참고)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$K = (Resolve-Path $Kmttt).Path
$env:KMTTT_SESSION_NAME = $Session
New-Item -ItemType Directory -Force $Work, $Out | Out-Null
$Work = (Resolve-Path $Work).Path
$Out = (Resolve-Path $Out).Path
$summary = Join-Path $Out "agents-summary.txt"
"KMttt real-agent check $(Get-Date -Format s)`n$(& $K --version)" | Set-Content $summary -Encoding utf8

function Log([string]$file, [string]$text) { Add-Content -Path (Join-Path $Out $file) -Value $text -Encoding utf8 }
function Kj { $j = ((& $K @args) | Out-String) | ConvertFrom-Json; $j | ForEach-Object { $_ } }
function Agents { try { @(Kj agent list) } catch { @() } }
function AgentOf([string]$pane) { Agents | Where-Object { $_.id -eq $pane } | Select-Object -First 1 }
function StateOf([string]$pane) { $a = AgentOf $pane; if ($a -and $a.agent) { "$($a.agent.kind)/$($a.agent.state)" } else { "-" } }
function Screen([string]$pane) { (& $K pane read $pane --source screen --lines 30) | Out-String }
# 질문 문장 자체에도 KMTTT-OK가 들어 있으므로, 질문 줄이 아닌 곳에 나온 경우만 답으로 본다.
function Answered([string]$pane) { @((Screen $pane) -split "`n" | Where-Object { $_ -match "KMTTT-OK" -and $_ -notmatch "Reply with" }).Count -gt 0 }

$candidates = @(
    @{ name = "claude"; cmd = "claude" },
    @{ name = "codex"; cmd = "codex" },
    @{ name = "gemini"; cmd = "gemini" },
    @{ name = "copilot"; cmd = "copilot" }
)
$prompt = "Reply with exactly the text KMTTT-OK and nothing else. Do not run any tools."

# -File로 부르면 "claude,codex"가 문자열 하나로 들어오므로 쉼표로 나눈다.
$Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
if ($Only.Count -gt 0) { $candidates = @($candidates | Where-Object { $Only -contains $_.name }) }
foreach ($c in $candidates) {
    $name = $c.name
    $file = "agent-$name.txt"
    $cmd = Get-Command $c.cmd -ErrorAction SilentlyContinue
    if (-not $cmd) { Add-Content $summary "$name : not installed"; continue }
    Log $file "== $name ($($cmd.Source))"
    & $K workspace create --label $name --cwd $Work --no-focus | Out-Null
    Start-Sleep -Seconds 3
    $pane = @(Kj pane list)[-1].id
    & $K pane run $pane $c.cmd | Out-Null
    # 시작 화면(로그인·폴더 신뢰 질문 등)을 기다리며 상태를 기록한다.
    $trace = @()
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Seconds 1
        $trace += "t+$($i+1)s $(StateOf $pane)"
    }
    Log $file "-- startup states"; Log $file ($trace -join "`n")
    Log $file "-- startup explain"; Log $file ((& $K agent explain $pane) | Out-String)
    Log $file "-- startup screen"; Log $file (Screen $pane)
    $state = StateOf $pane
    if ($state -like "*/blocked") {
        # 시작 질문에 안전하게 답한다. 기본값으로 Enter를 누르지 않는다
        # (Codex 업데이트 메뉴는 기본 선택이 "Update now"라 Enter가 전역 업데이트를 실행한다).
        $scr = Screen $pane
        if ($scr -match "Yes, I trust") {
            # Claude Code 폴더 신뢰 질문: 기본 선택이 "No, exit"라 아래로 옮겨 "Yes"를 고른다(시험용 빈 폴더).
            Log $file "-- startup trust prompt: choosing 'Yes, I trust this folder'"
            & $K pane send $pane '\x1b[B' | Out-Null
            Start-Sleep -Milliseconds 500
            & $K pane send $pane '\r' | Out-Null
        } elseif ($scr -match "(?i)esc (to )?skip|esc to cancel") {
            Log $file "-- startup menu: skipping it with Esc"
            & $K pane send $pane '\x1b' | Out-Null
        } else {
            Log $file "-- startup question not recognised; leaving it for a person"
        }
        Start-Sleep -Seconds 6
    }
    if ((StateOf $pane) -eq "-") {
        Add-Content $summary "$name : NOT DETECTED as an agent (see $file)"
        & $K pane send $pane '\x03' | Out-Null
        continue
    }
    if ((StateOf $pane) -like "*/blocked") {
        Add-Content $summary "$name : still asking a start-up question; prompt not sent (see $file)"
        Log $file "-- still blocked; screen:"; Log $file (Screen $pane)
        continue
    }
    # 짧은 질문을 보내고 상태 변화를 따라간다(blocked면 kmttt가 거부한다).
    & $K agent prompt $pane $prompt | Out-Null
    $trace = @()
    $seen = @{}
    for ($i = 0; $i -lt 90; $i++) {
        Start-Sleep -Seconds 1
        $s = StateOf $pane
        $trace += "t+$($i+1)s $s"
        $seen[$s] = $true
        if ($i -gt 5 -and ($s -like "*/done" -or $s -like "*/idle" -or $s -like "*/blocked") -and (Answered $pane)) { break }
    }
    Log $file "-- states after prompt"; Log $file ($trace -join "`n")
    Log $file "-- explain"; Log $file ((& $K agent explain $pane) | Out-String)
    Log $file "-- final screen"; Log $file (Screen $pane)
    $answered = Answered $pane
    Add-Content $summary ("{0} : detected={1} states=[{2}] answered={3}" -f $name, ((AgentOf $pane).agent.kind), (($seen.Keys | Sort-Object) -join ", "), $answered)
}
Add-Content $summary "done $(Get-Date -Format s)"
# 관제 화면을 새 창으로 띄워 둔다(사람이 보거나 스크린샷을 찍도록). 에이전트는 계속 떠 있다.
Start-Process -FilePath $K -ArgumentList "--session $Session --control"
