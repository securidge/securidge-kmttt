# 저장소 루트에서 실행: & ([scriptblock]::Create((Get-Content -Encoding UTF8 ci/test-e2e-windows-isolation.ps1 -Raw)))
# 실제 kmttt 호출 없이 세션 선택·환경 전달·오류 시 정리를 검증한다.
$ErrorActionPreference = 'Stop'
$source = Get-Content -LiteralPath (Join-Path $PWD 'ci/e2e-windows.ps1') -Raw -Encoding UTF8
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
foreach ($name in @('New-E2EContext', 'Remove-E2ETempRoot', 'New-E2EWmiCommand', 'Assert-E2EExternalTerminal', 'Invoke-E2ERedirectedList')) {
    $fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true)
    if (-not $fn) { throw "Missing function $name" }
    . ([scriptblock]::Create($fn.Extent.Text))
}
function Assert($condition, $message) { if (-not $condition) { throw $message } }
$envKeys = @('GITHUB_ACTIONS', 'KMTTT_CONFIG_DIR', 'KMTTT_DATA_DIR', 'KMTTT_SOCKET_DIR', 'KMTTT_SESSION_NAME', 'KMTTT_PANE')
$originalEnv = @{}
foreach ($key in $envKeys) { $originalEnv[$key] = [Environment]::GetEnvironmentVariable($key) }
$contexts = @()
try {
    $paneBefore = $env:KMTTT_PANE
    $paneRefused = $false
    try { Assert-E2EExternalTerminal 'fixture-pane' } catch { $paneRefused = $_.ToString() -match 'KMttt 밖 터미널' }
    Assert $paneRefused 'E2E must refuse a KMttt pane'
    Assert ($env:KMTTT_PANE -eq $paneBefore) 'Entry guard must not clear the caller identity'
    if ($env:KMTTT_PANE) {
        $scriptRefused = $false
        try { & ([scriptblock]::Create($source)) -Kmttt nonexistent } catch { $scriptRefused = $_.ToString() -match 'KMttt 밖 터미널' }
        Assert $scriptRefused 'Pane guard must run before executable lookup or setup'
    }
    Assert-E2EExternalTerminal ''

    $one = New-E2EContext '' $false $false
    $contexts += $one
    $two = New-E2EContext '' $false $false
    $contexts += $two
    Assert ($one.Session -match '^e2e-[a-f0-9]{32}$' -and $one.Session -ne $two.Session) 'Local sessions must be random and distinct'
    Assert ($one.Root -ne $two.Root -and (Test-Path -LiteralPath $one.Root)) 'Local directories must be distinct'
    $ci = New-E2EContext '' $true $false
    Assert ($ci.Session -eq 'default' -and -not $ci.Root) 'CI defaults changed'
    $ciCustom = New-E2EContext 'ci-test' $true $false
    Assert ($ciCustom.Session -eq 'ci-test' -and -not $ciCustom.Root) 'CI explicit session changed'
    $forced = New-E2EContext 'default' $false $true
    $contexts += $forced
    Assert ($forced.Session -eq 'default' -and $forced.Root) 'Force must still use isolated directories'
    $badSessionRejected = $false
    try { New-E2EContext '../default' $false $true | Out-Null } catch { $badSessionRejected = $true }
    Assert $badSessionRejected 'Invalid session must be rejected'
    $unsafeRemovalRejected = $false
    try { Remove-E2ETempRoot ([IO.Path]::GetTempPath()) } catch { $unsafeRemovalRejected = $true }
    Assert $unsafeRemovalRejected 'Cleanup must refuse a directory it does not own'

    $env:KMTTT_CONFIG_DIR = Join-Path $one.Root "config with 'quote"
    $env:KMTTT_DATA_DIR = Join-Path $one.Root 'data with spaces'
    $env:KMTTT_SOCKET_DIR = Join-Path $one.Root 'socket'
    $env:KMTTT_SESSION_NAME = $one.Session
    $payload = 'Write-Output isolation-marker'
    $wmi = New-E2EWmiCommand $payload $false
    $decoded = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String(($wmi -split ' ')[-1]))
    Assert ($decoded.Contains("config with ''quote")) 'WMI paths must escape apostrophes'
    foreach ($key in @('KMTTT_CONFIG_DIR', 'KMTTT_DATA_DIR', 'KMTTT_SOCKET_DIR', 'KMTTT_SESSION_NAME')) {
        Assert ($decoded.Contains('$env:' + $key + ' = ')) "WMI must explicitly pass $key"
    }
    Assert ((& ([scriptblock]::Create($decoded))) -eq 'isolation-marker') 'Encoded WMI setup must be executable'
    Assert ((New-E2EWmiCommand $payload $true) -eq $payload) 'CI WMI command must remain unchanged'

    $fixture = Join-Path $one.Root 'fixture app.cmd'
    $redirect = Join-Path $one.Root 'redirect output.txt'
    Set-Content -LiteralPath $fixture -Value "@echo off`r`necho %*" -Encoding ASCII
    Invoke-E2ERedirectedList $fixture 'fixture-session' $redirect
    Assert ((Get-Content -LiteralPath $redirect -Raw).Trim() -eq '--session fixture-session workspace list') 'cmd redirection must preserve spaced executable and output paths'
    Remove-Item -LiteralPath $redirect -ErrorAction Stop

    # 실제 앱 대신 3초 뒤 끝나는 자식을 띄워 stdout 핸들을 의도적으로 상속시킨다.
    # 기존 PowerShell 리다이렉트에서는 파일을 바로 지울 수 있어 이 회귀를 놓쳤다.
    Set-Content -LiteralPath $fixture -Encoding ASCII -Value '@echo off', 'start "" /b "%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -Command "Start-Sleep -Seconds 3"'
    Invoke-E2ERedirectedList $fixture 'fixture-session' $redirect
    $inheritedHandleDetected = $false
    try { Remove-Item -LiteralPath $redirect -ErrorAction Stop } catch { $inheritedHandleDetected = $true }
    Assert $inheritedHandleDetected 'Redirect leak check must detect a live child holding the output handle'
    Start-Sleep -Seconds 4
    Remove-Item -LiteralPath $redirect -ErrorAction Stop

    # 첫 서버 호출에서 실패하게 하여 실제 스크립트의 finally와 환경 복원을 확인한다.
    # 아래 실행은 모든 kmttt 호출을 mock으로 대체한다. 이 테스트 범위에서만 진입 가드를 mock한다.
    # 실제 KMTTT_PANE 값은 지우거나 바꾸지 않는다.
    $guard = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Assert-E2EExternalTerminal' }, $true)
    $mockSource = $source.Remove($guard.Extent.StartOffset, $guard.Extent.EndOffset - $guard.Extent.StartOffset)
    function Assert-E2EExternalTerminal { }
    $env:GITHUB_ACTIONS = 'false'
    $saved = @{}
    foreach ($key in $envKeys) { $saved[$key] = [Environment]::GetEnvironmentVariable($key) }
    $calls = New-Object 'System.Collections.Generic.List[object]'
    function Resolve-Path { [pscustomobject]@{ Path = 'Invoke-FakeKmttt' } }
    function Get-CimInstance { return $null }
    function Invoke-FakeKmttt {
        $calls.Add(@{ Args = @($args); Session = $env:KMTTT_SESSION_NAME; Config = $env:KMTTT_CONFIG_DIR; Data = $env:KMTTT_DATA_DIR; Socket = $env:KMTTT_SOCKET_DIR })
        if ($args[0] -eq 'workspace') { throw 'injected-e2e-failure' }
    }
    $refused = $false
    try { & ([scriptblock]::Create($mockSource)) -Kmttt unused -Session default } catch { $refused = $_.ToString() -match 'refuses session default' }
    Assert ($refused -and $calls.Count -eq 0) 'Default must be refused before any server command'
    $failedAsExpected = $false
    try { & ([scriptblock]::Create($mockSource)) -Kmttt unused } catch { $failedAsExpected = $_.ToString() -match 'injected-e2e-failure' }
    Assert $failedAsExpected 'Failure injection did not run'
    Assert ($calls.Count -eq 2) 'Failure must attempt exactly one isolated server stop'
    $start = $calls[0]
    $stop = $calls[1]
    Assert (($stop.Args -join ' ') -eq "--session $($start.Session) server stop") 'Cleanup must stop only the selected session'
    Assert ($start.Session -ne 'default' -and $start.Config -ne $saved.KMTTT_CONFIG_DIR) 'Local run must override inherited settings'
    Assert ($start.Data -eq $stop.Data -and $start.Socket -eq $stop.Socket) 'Cleanup must keep the isolated server endpoint'
    Assert (-not (Test-Path -LiteralPath (Split-Path $start.Data))) 'Failed run left its temporary directory behind'
    foreach ($key in $envKeys) {
        Assert ([Environment]::GetEnvironmentVariable($key) -eq $saved[$key]) "Environment not restored: $key"
    }
    'PASS: pane refusal without identity changes, cmd redirection with spaced paths, inherited handle leak detected, local defaults, default refusal, Force, CI parity, WMI paths, failure cleanup, environment restoration'
} finally {
    foreach ($context in $contexts) { Remove-E2ETempRoot $context.Root }
    foreach ($key in $envKeys) { [Environment]::SetEnvironmentVariable($key, $originalEnv[$key]) }
}
