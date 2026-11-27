//! 알림 전달: 앱 안(상태 줄), 바깥 터미널(OSC 9 + 종), 운영체제 알림.

use std::io::Write;

pub fn deliver(delivery: &str, text: &str) {
    match delivery {
        "terminal" => {
            // OSC 9: iTerm2·Windows Terminal·WezTerm 등이 데스크톱 알림으로 보여 준다.
            let clean: String = text.chars().filter(|c| !c.is_control()).collect();
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b]9;{clean}\x07\x07");
            let _ = out.flush();
        }
        "os" => os_notify(text),
        _ => {}
    }
}

fn os_notify(text: &str) {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\'' && *c != '`' && *c != '$')
        .collect();
    #[cfg(target_os = "linux")]
    let cmd = std::process::Command::new("notify-send")
        .arg("Securidge KMttt")
        .arg(&clean)
        .spawn();
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "display notification \"{clean}\" with title \"Securidge KMttt\""
        ))
        .spawn();
    #[cfg(windows)]
    let cmd = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
                 $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
                 $t.GetElementsByTagName('text')[0].AppendChild($t.CreateTextNode('Securidge KMttt')) | Out-Null; \
                 $t.GetElementsByTagName('text')[1].AppendChild($t.CreateTextNode('{clean}')) | Out-Null; \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Securidge KMttt').Show([Windows.UI.Notifications.ToastNotification]::new($t))"
            ),
        ])
        .spawn();
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let cmd: std::io::Result<std::process::Child> = Err(std::io::Error::other("unsupported"));
    let _ = cmd;
}
