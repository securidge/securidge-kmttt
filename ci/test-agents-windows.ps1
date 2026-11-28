# 실행 정책/사용자 설정 변경이나 실제 에이전트 실행 없이 명령 선택을 검증한다.
# 실행: & ([scriptblock]::Create((Get-Content ci/test-agents-windows.ps1 -Raw)))
$ErrorActionPreference = 'Stop'
$source = Join-Path $PWD 'ci/agents-windows.ps1'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($source, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
$resolver = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Resolve-AgentCommand' }, $true)
if (-not $resolver) { throw 'Resolve-AgentCommand not found' }
. ([scriptblock]::Create($resolver.Extent.Text))

$originalPath = $env:PATH
$root = Join-Path $PWD ('.agent-command-test-' + [guid]::NewGuid().ToString('N'))
$first = Join-Path $root "first agent's folder"
$second = Join-Path $root 'second folder'
$name = 'kmttt-agent-fixture'
try {
    New-Item -ItemType Directory $first, $second | Out-Null
    $env:PATH = "$first;$second"
    Set-Content -LiteralPath (Join-Path $first "$name.ps1") -Value 'throw "ps1 must not run"'
    if (Resolve-AgentCommand $name) { throw '.ps1-only install must not be selected' }
    if (Resolve-AgentCommand 'kmttt-agent-missing') { throw 'Missing agent must not resolve' }

    $cmdPath = Join-Path $first "$name.cmd"
    Set-Content -LiteralPath $cmdPath -Value '@echo fixture-ok'
    # 수정 전 Get-Command가 .ps1을 선택하는지 확인한다(실행하지 않음).
    if ((Get-Command $name).CommandType -ne 'ExternalScript') { throw 'Fixture did not reproduce original .ps1 selection' }
    $cmd = Resolve-AgentCommand $name
    if ($cmd.Source -ne $cmdPath) { throw '.cmd fallback failed' }
    $launch = "& '" + $cmd.Source.Replace("'", "''") + "'"
    if ((& ([scriptblock]::Create($launch))) -ne 'fixture-ok') { throw 'Quoted .cmd path failed to run' }

    # .cmd가 PATH 앞에 있어도 뒤쪽의 .exe를 우선한다.
    $exePath = Join-Path $second "$name.exe"
    New-Item -ItemType File $exePath | Out-Null
    if ((Resolve-AgentCommand $name).Source -ne $exePath) { throw '.exe must take priority over .cmd' }
    'PASS: original .ps1 regression, .cmd fallback, .exe priority, missing agent, quoted path'
} finally {
    $env:PATH = $originalPath
    # 만든 파일과 빈 디렉터리만 지운다(재귀 삭제하지 않음).
    foreach ($file in @("$name.ps1", "$name.cmd")) {
        Remove-Item -LiteralPath (Join-Path $first $file) -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath (Join-Path $second "$name.exe") -ErrorAction SilentlyContinue
    foreach ($dir in @($first, $second, $root)) {
        if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir }
    }
}
