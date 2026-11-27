//! 로컬 IPC: Unix 소켓(폴더 0700, 소켓 0600) / Windows named pipe(현재 사용자 SID만 허용하는 DACL).
//! 접속마다 상대 프로세스 PID를 OS에서 얻는다(호출자 식별, F1).

use crate::Server;
use anyhow::Result;
use std::sync::Arc;

/// 같은 세션 서버가 이미 떠 있는지.
pub async fn server_running(session: &str) -> bool {
    kmttt_core::client::RpcClient::connect_local(session)
        .await
        .is_ok()
}

#[cfg(unix)]
pub async fn serve(server: Arc<Server>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;
    let dir = kmttt_core::paths::socket_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = kmttt_core::paths::socket_path(&server.opts.session);
    if std::path::Path::new(&path).exists() {
        // server_running()이 false였으니 남은 소켓 파일이다.
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!("listening on {path}");
    loop {
        tokio::select! {
            r = listener.accept() => {
                let Ok((stream, _)) = r else { continue };
                let pid = stream.peer_cred().ok().and_then(|c| c.pid()).map(|p| p as u32);
                let (rd, wr) = stream.into_split();
                let s = server.clone();
                tokio::spawn(async move { crate::rpc::serve_conn(s, rd, wr, pid).await });
            }
            _ = server.shutdown.notified() => break,
        }
    }
    let _ = std::fs::remove_file(&path);
    Ok(())
}

#[cfg(windows)]
pub async fn serve(server: Arc<Server>) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = kmttt_core::paths::socket_path(&server.opts.session);
    let sa = win::SecurityAttrs::current_user_only()?;
    // SAFETY: sa는 이 함수가 끝날 때까지 살아 있고, 보안 설명자는 유효한 SDDL에서 만들었다.
    let mut pipe = unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .create_with_security_attributes_raw(&name, sa.as_ptr())?
    };
    tracing::info!("listening on {name}");
    loop {
        tokio::select! {
            r = pipe.connect() => {
                if r.is_err() { continue; }
                let connected = pipe;
                // SAFETY: 위와 같다.
                pipe = unsafe { ServerOptions::new().create_with_security_attributes_raw(&name, sa.as_ptr())? };
                let pid = win::client_pid(&connected);
                let (rd, wr) = tokio::io::split(connected);
                let s = server.clone();
                tokio::spawn(async move { crate::rpc::serve_conn(s, rd, wr, pid).await });
            }
            _ = server.shutdown.notified() => break,
        }
    }
    Ok(())
}

#[cfg(windows)]
mod win {
    use anyhow::{bail, Result};
    use std::os::windows::io::AsRawHandle;
    use tokio::net::windows::named_pipe::NamedPipeServer;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub struct SecurityAttrs {
        sa: Box<SECURITY_ATTRIBUTES>,
        sd: PSECURITY_DESCRIPTOR,
    }

    // SAFETY: 보안 설명자는 만든 뒤 읽기만 한다.
    unsafe impl Send for SecurityAttrs {}
    unsafe impl Sync for SecurityAttrs {}

    impl Drop for SecurityAttrs {
        fn drop(&mut self) {
            // SAFETY: sd는 ConvertStringSecurityDescriptorToSecurityDescriptorW가 LocalAlloc으로 준 것이다.
            unsafe {
                LocalFree(self.sd as _);
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 현재 사용자의 SID 문자열(S-1-5-21-…).
    fn current_user_sid() -> Result<String> {
        // SAFETY: 토큰 핸들과 버퍼는 이 함수 안에서 만들고 닫는다. 버퍼 길이는 첫 호출로 얻는다.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                bail!("OpenProcessToken: {}", std::io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as _, len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                bail!("GetTokenInformation: {}", std::io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut pw: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut pw) == 0 {
                bail!(
                    "ConvertSidToStringSidW: {}",
                    std::io::Error::last_os_error()
                );
            }
            let mut n = 0;
            while *pw.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(pw, n));
            LocalFree(pw as _);
            Ok(s)
        }
    }

    impl SecurityAttrs {
        /// 보호된 DACL: 현재 사용자와 SYSTEM만 접근. 상속 ACE 없음.
        pub fn current_user_only() -> Result<Self> {
            let sid = current_user_sid()?;
            let sddl = wide(&format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: sddl은 NUL로 끝나는 UTF-16 문자열이다.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                bail!(
                    "ConvertStringSecurityDescriptorToSecurityDescriptorW: {}",
                    std::io::Error::last_os_error()
                );
            }
            let sa = Box::new(SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            });
            Ok(Self { sa, sd })
        }

        pub fn as_ptr(&self) -> *mut core::ffi::c_void {
            &*self.sa as *const SECURITY_ATTRIBUTES as *mut core::ffi::c_void
        }
    }

    pub fn client_pid(pipe: &NamedPipeServer) -> Option<u32> {
        let mut pid = 0u32;
        // SAFETY: 연결된 파이프 핸들이며 pid는 출력 버퍼다.
        let ok = unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle() as HANDLE, &mut pid) };
        (ok != 0 && pid != 0).then_some(pid)
    }
}
