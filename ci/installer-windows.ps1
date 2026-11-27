# KMttt Desktop Windows 설치 프로그램 확인(릴리스 워크플로에서 실행).
# 설치 → 파일·제거 정보 확인 → 사용량 상태 줄 등록 → 서버 실행 중 업그레이드(서버를 끄고 덮어쓰는지)
# → 서버 실행 중 제거(서버 종료·훅 정리·파일 삭제·설정 보존) 순서로 본다.
param([Parameter(Mandatory = $true)][string]$Setup)
$ErrorActionPreference = 'Stop'
$fail = 0
function Check($ok, $what) {
    if ($ok) { Write-Host "ok   - $what" } else { Write-Host "FAIL - $what"; $script:fail++ }
}
function ShowProcs {
    Get-CimInstance Win32_Process -Filter "Name='kmttt.exe'" | ForEach-Object { Write-Host "     running: pid $($_.ProcessId) $($_.ExecutablePath)" }
}
function ServerProcs($dir) {
    @(Get-Process kmttt -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $dir 'kmttt.exe') })
}

$dir = Join-Path $env:LOCALAPPDATA 'KMttt Desktop'
$tmp = Join-Path $env:RUNNER_TEMP 'kmttt-inst'
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$settings = Join-Path $tmp 'claude-settings.json'
Set-Content -Path $settings -Value '{"model":"x","statusLine":{"type":"command","command":"mine.sh"}}' -Encoding ascii
$env:KMTTT_CLAUDE_SETTINGS = $settings

# 1. 조용한 설치
Start-Process -Wait -FilePath $Setup -ArgumentList '/S'
Check (Test-Path (Join-Path $dir 'kmttt-desktop.exe')) 'kmttt-desktop.exe installed'
Check (Test-Path (Join-Path $dir 'kmttt.exe')) 'kmttt.exe installed next to the app'
Check (Test-Path (Join-Path $dir 'uninstall.exe')) 'uninstaller present'
$reg = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' | ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.DisplayName -like 'KMttt Desktop*' }
Check ($null -ne $reg) 'registered in Apps & features (current user)'
if ($reg) { Write-Host "     publisher: $($reg.Publisher)"; Check ($reg.Publisher -like '*personal project*') 'publisher says personal project' }

# 2. 사용량 상태 줄(다른 상태 줄이 있으면 --replace 필요, 경로에 공백이 없어야 함)
& (Join-Path $dir 'kmttt.exe') integration install claude-usage --replace | Out-Host
$cmd = (Get-Content $settings -Raw | ConvertFrom-Json).statusLine.command
Write-Host "     statusLine: $cmd"
Check ($cmd -like '*statusline' -and $cmd -notlike '*KMttt Desktop*') 'status line command has no spaces (8.3 path) and ends with statusline'
$out = '{"rate_limits":{"five_hour":{"used_percentage":12,"resets_at":2000000000}}}' | & (Join-Path $dir 'kmttt.exe') statusline
Check ($out -like '*5h 12%*') "statusline prints usage ($out)"

# 3. 서버가 도는 중에 업그레이드(같은 설치 파일 다시 실행)
Start-Process -FilePath (Join-Path $dir 'kmttt.exe') -ArgumentList '--session', 'instcheck', 'server', 'run' -WindowStyle Hidden
Start-Sleep -Seconds 3
Check ((ServerProcs $dir).Count -ge 1) 'server running from the install folder'
Start-Process -Wait -FilePath $Setup -ArgumentList '/S'
Start-Sleep -Seconds 1
ShowProcs
Check ((ServerProcs $dir).Count -eq 0) 'upgrade stopped the running server'
Check (Test-Path (Join-Path $dir 'kmttt.exe')) 'kmttt.exe still installed after upgrade'

# 4. 서버가 도는 중에 제거
Start-Process -FilePath (Join-Path $dir 'kmttt.exe') -ArgumentList '--session', 'instcheck', 'server', 'run' -WindowStyle Hidden
Start-Sleep -Seconds 3
New-Item -ItemType Directory -Force -Path (Join-Path $env:APPDATA 'Securidge\kmttt') | Out-Null
Set-Content -Path (Join-Path $env:APPDATA 'Securidge\kmttt\policy.toml') -Value '# keep me'
Start-Process -Wait -FilePath (Join-Path $dir 'uninstall.exe') -ArgumentList '/S', "_?=$dir"
Start-Sleep -Seconds 1
ShowProcs
Check ((ServerProcs $dir).Count -eq 0) 'uninstall stopped the running server'
Check (-not (Test-Path (Join-Path $dir 'kmttt-desktop.exe'))) 'app removed'
Check (-not (Test-Path (Join-Path $dir 'kmttt.exe'))) 'kmttt.exe removed'
$after = (Get-Content $settings -Raw | ConvertFrom-Json).statusLine.command
Check ($after -eq 'mine.sh') "uninstall restored the user's own status line ($after)"
$reg2 = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' | ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.DisplayName -like 'KMttt Desktop*' }
Check ($null -eq $reg2) 'removed from Apps & features'
Check (Test-Path (Join-Path $env:APPDATA 'Securidge\kmttt\policy.toml')) 'silent uninstall keeps settings and the security log (default No)'

if ($fail -gt 0) { Write-Host "$fail check(s) failed"; exit 1 }
Write-Host 'installer checks passed'
