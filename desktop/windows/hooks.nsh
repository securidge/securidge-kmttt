; KMttt Desktop 설치 프로그램 훅 (Tauri NSIS installerHooks).
; - 설치(업그레이드)·제거 전에: 이 폴더의 kmttt.exe로 돌고 있는 KMttt 서버가 있으면 묻고 끈다.
;   (서버가 돌고 있으면 kmttt.exe를 바꾸거나 지울 수 없다.)
; - 제거 전에: 이 kmttt가 Claude Code·Codex 설정에 등록한 훅·상태 줄을 지운다(다른 위치의 kmttt가 등록한 것은 그대로).
; - 라벨은 같은 줄 번호 매크로(${__LINE__})로 만들면 줄마다 값이 달라 어긋나므로, 넣는 곳마다 ID를 넘긴다.
; - 설치 프로그램은 32비트라 거기서 띄운 PowerShell의 Get-Process는 64비트 kmttt의 경로(Path)를 못 읽는다.
;   그래서 WMI(Win32_Process.ExecutablePath)로 찾는다.
; - 제거 뒤에: 설정·보안 기록을 지울지 묻는다(기본은 남김, 조용한 제거(/S)에서도 남김).

!macro KMTTT_STOP_SERVERS ID
  nsExec::ExecToStack `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "if (@(Get-CimInstance Win32_Process | Where-Object { $$_.Name -eq 'kmttt.exe' -and $$_.ExecutablePath -eq '$INSTDIR\kmttt.exe' }).Count -gt 0) { exit 3 } else { exit 0 }"`
  Pop $0
  Pop $1
  StrCmp $0 "3" 0 kmttt_stop_done_${ID}
    MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "KMttt 서버가 실행 중입니다. 계속하면 서버와 열려 있는 모든 터미널·AI 에이전트가 종료됩니다.$\r$\n$\r$\nThe KMttt server is running. Continuing stops it and every open terminal and AI agent." /SD IDOK IDOK kmttt_stop_go_${ID}
    Abort
  kmttt_stop_go_${ID}:
    nsExec::ExecToStack `"$INSTDIR\kmttt.exe" server stop`
    Pop $0
    Pop $1
    Sleep 800
    nsExec::ExecToStack `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "Get-CimInstance Win32_Process | Where-Object { $$_.Name -eq 'kmttt.exe' -and $$_.ExecutablePath -eq '$INSTDIR\kmttt.exe' } | ForEach-Object { Stop-Process -Id $$_.ProcessId -Force }"`
    Pop $0
    Pop $1
    Sleep 500
  kmttt_stop_done_${ID}:
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro KMTTT_STOP_SERVERS inst
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro KMTTT_STOP_SERVERS uninst
  IfFileExists "$INSTDIR\kmttt.exe" 0 kmttt_no_exe
    nsExec::ExecToStack `"$INSTDIR\kmttt.exe" integration uninstall claude-usage --if-this-exe`
    Pop $0
    Pop $1
    nsExec::ExecToStack `"$INSTDIR\kmttt.exe" integration uninstall claude --if-this-exe`
    Pop $0
    Pop $1
    nsExec::ExecToStack `"$INSTDIR\kmttt.exe" integration uninstall codex --if-this-exe`
    Pop $0
    Pop $1
  kmttt_no_exe:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "KMttt 설정(보안 규칙 포함)과 보안 기록도 지울까요?$\r$\n보안 기록은 나중에 무슨 일이 있었는지 확인할 증거입니다. 잘 모르겠으면 [아니요]를 누르세요.$\r$\n$\r$\nAlso delete KMttt settings (including security rules) and the security log?$\r$\nThe log is evidence you may need later. If unsure, choose No.$\r$\n$\r$\n$APPDATA\Securidge\kmttt$\r$\n$LOCALAPPDATA\Securidge\kmttt" /SD IDNO IDNO kmttt_keep_data
    RMDir /r "$APPDATA\Securidge\kmttt"
    RMDir /r "$LOCALAPPDATA\Securidge\kmttt"
    RMDir "$APPDATA\Securidge"
    RMDir "$LOCALAPPDATA\Securidge"
  kmttt_keep_data:
!macroend
