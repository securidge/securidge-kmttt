//! 세션 서버를 띄우고 붙는다. CLI와 데스크톱 앱이 함께 쓴다.

use crate::client::{Notifications, RpcClient};
use crate::paths;
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::time::Duration;

/// 서버에 붙는다. 없으면 `exe`(kmttt 실행 파일)로 서버를 띄운 뒤 붙는다.
pub async fn ensure_server(exe: &Path, session: &str) -> Result<(RpcClient, Notifications)> {
    if let Ok(c) = RpcClient::connect_local(session).await {
        return Ok(c);
    }
    spawn_server(exe, session)?;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(c) = RpcClient::connect_local(session).await {
            return Ok(c);
        }
    }
    Err(anyhow!(
        "the server did not start; see {}",
        paths::log_file().display()
    ))
}

/// `exe --session <session> server run`을 호출한 쪽과 떨어진 프로세스로 띄운다.
pub fn spawn_server(exe: &Path, session: &str) -> Result<()> {
    anyhow::ensure!(
        paths::valid_session_name(session),
        "invalid session name '{session}'"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["--session", session, "server", "run"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // 터미널을 닫아도 서버가 남도록 새 프로세스 그룹으로.
            .process_group(0);
        cmd.spawn().context("start server")?;
        Ok(())
    }
    #[cfg(windows)]
    {
        spawn_server_windows(exe, session)
    }
}

/// Windows: 핸들을 하나도 물려주지 않고(bInheritHandles = FALSE) 서버를 띄운다.
///
/// std의 Command는 상속 가능한 핸들을 모두 자식에게 넘긴다. 그러면 이 CLI를 부른 셸이 넘겨준
/// 파이프·리다이렉트 파일 핸들까지 오래 사는 서버가 쥐게 되어, 호출한 쪽이 출력 끝을 못 받거나
/// 그 파일을 지우거나 다시 쓸 수 없게 된다(실제 Windows 노트북에서 발견).
#[cfg(windows)]
fn spawn_server_windows(exe: &Path, session: &str) -> Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, PROCESS_INFORMATION,
        STARTUPINFOW,
    };
    // 세션 이름은 영숫자·-·_ 만 허용되어(valid_session_name) 따옴표 처리가 필요 없다.
    let cmdline = format!("\"{}\" --session {session} server run", exe.display());
    let mut wide: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: 구조체는 0으로 초기화한 뒤 크기만 채우고, 문자열 버퍼는 호출 동안 살아 있다.
    // 성공하면 받은 프로세스·스레드 핸들을 바로 닫는다(서버는 계속 돈다).
    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessW(
            std::ptr::null(),
            wide.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        );
        if ok == 0 {
            return Err(std::io::Error::last_os_error()).context("start server");
        }
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }
    Ok(())
}
