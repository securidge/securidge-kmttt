//! 키·마우스 이벤트 → 터미널 입력 바이트(xterm 방식), 그리고 키바인딩 비교용 KeySpec 변환.

use kmttt_core::keys::{KeyName, KeySpec};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// crossterm 키를 바인딩 비교용으로 바꾼다.
pub fn to_spec(k: &KeyEvent, after_prefix: bool) -> Option<KeySpec> {
    let m = k.modifiers;
    let mut shift = m.contains(KeyModifiers::SHIFT);
    let key = match k.code {
        KeyCode::Char(c) => {
            if c.is_ascii_uppercase() {
                shift = true;
            }
            // shift가 붙은 기호(?, + 등)는 문자 자체로 비교한다.
            if !c.is_ascii_alphabetic() {
                shift = false;
            }
            KeyName::Char(c.to_ascii_lowercase())
        }
        KeyCode::Enter => KeyName::Enter,
        KeyCode::Esc => KeyName::Esc,
        KeyCode::Tab => KeyName::Tab,
        KeyCode::Backspace => KeyName::Backspace,
        KeyCode::Left => KeyName::Left,
        KeyCode::Right => KeyName::Right,
        KeyCode::Up => KeyName::Up,
        KeyCode::Down => KeyName::Down,
        KeyCode::F(n) => KeyName::F(n),
        _ => return None,
    };
    Some(KeySpec {
        after_prefix,
        ctrl: m.contains(KeyModifiers::CONTROL),
        alt: m.contains(KeyModifiers::ALT),
        shift,
        key,
    })
}

/// 바인딩 KeySpec이 이 키와 같은지(after_prefix 무시).
pub fn same_key(spec: &KeySpec, k: &KeyEvent) -> bool {
    to_spec(k, spec.after_prefix).as_ref() == Some(spec)
}

fn modifier_param(m: KeyModifiers) -> u8 {
    1 + if m.contains(KeyModifiers::SHIFT) {
        1
    } else {
        0
    } + if m.contains(KeyModifiers::ALT) { 2 } else { 0 }
        + if m.contains(KeyModifiers::CONTROL) {
            4
        } else {
            0
        }
}

/// 키 하나를 PTY로 보낼 바이트로. `app_cursor`는 응용 커서 모드(DECCKM).
pub fn encode_key(k: &KeyEvent, app_cursor: bool) -> Vec<u8> {
    let m = k.modifiers;
    let alt = m.contains(KeyModifiers::ALT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let mut out = Vec::new();
    let csi_mod = |final_: char, out: &mut Vec<u8>| {
        let p = modifier_param(m);
        if p > 1 {
            out.extend(format!("\x1b[1;{p}{final_}").as_bytes());
        } else if app_cursor {
            out.extend(format!("\x1bO{final_}").as_bytes());
        } else {
            out.extend(format!("\x1b[{final_}").as_bytes());
        }
    };
    let tilde = |n: u8, out: &mut Vec<u8>| {
        let p = modifier_param(m);
        if p > 1 {
            out.extend(format!("\x1b[{n};{p}~").as_bytes());
        } else {
            out.extend(format!("\x1b[{n}~").as_bytes());
        }
    };
    match k.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                let b = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => c as u8 - b'a' + 1,
                    ' ' | '@' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '7' | '-' => 0x1f,
                    '?' | '8' => 0x7f,
                    other => {
                        let mut buf = [0u8; 4];
                        out.extend(other.encode_utf8(&mut buf).as_bytes());
                        return out;
                    }
                };
                out.push(b);
            } else {
                let mut buf = [0u8; 4];
                out.extend(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend(b"\x1b[Z"),
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => csi_mod('A', &mut out),
        KeyCode::Down => csi_mod('B', &mut out),
        KeyCode::Right => csi_mod('C', &mut out),
        KeyCode::Left => csi_mod('D', &mut out),
        KeyCode::Home => csi_mod('H', &mut out),
        KeyCode::End => csi_mod('F', &mut out),
        KeyCode::Insert => tilde(2, &mut out),
        KeyCode::Delete => tilde(3, &mut out),
        KeyCode::PageUp => tilde(5, &mut out),
        KeyCode::PageDown => tilde(6, &mut out),
        KeyCode::F(n) => match n {
            1..=4 => out.extend(format!("\x1bO{}", (b'P' + n - 1) as char).as_bytes()),
            5 => tilde(15, &mut out),
            6..=10 => tilde(17 + (n - 6), &mut out),
            11 => tilde(23, &mut out),
            12 => tilde(24, &mut out),
            _ => {}
        },
        _ => {}
    }
    out
}

/// 붙여넣기. 응용이 bracketed paste를 켰으면 감싼다.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// SGR 방식 마우스 휠(위 64, 아래 65). x, y는 pane 안 0부터.
pub fn encode_wheel(up: bool, x: u16, y: u16) -> Vec<u8> {
    format!("\x1b[<{};{};{}M", if up { 64 } else { 65 }, x + 1, y + 1).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventKind;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: m,
            kind: KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn encodes() {
        assert_eq!(
            encode_key(&k(KeyCode::Char('a'), KeyModifiers::NONE), false),
            b"a"
        );
        assert_eq!(
            encode_key(&k(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            [3]
        );
        assert_eq!(
            encode_key(&k(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
        assert_eq!(
            encode_key(&k(KeyCode::Up, KeyModifiers::NONE), false),
            b"\x1b[A"
        );
        assert_eq!(
            encode_key(&k(KeyCode::Up, KeyModifiers::NONE), true),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(&k(KeyCode::Right, KeyModifiers::CONTROL), false),
            b"\x1b[1;5C"
        );
        assert_eq!(
            encode_key(&k(KeyCode::F(5), KeyModifiers::NONE), false),
            b"\x1b[15~"
        );
        assert_eq!(
            encode_key(&k(KeyCode::F(1), KeyModifiers::NONE), false),
            b"\x1bOP"
        );
        assert_eq!(
            encode_key(&k(KeyCode::Enter, KeyModifiers::NONE), false),
            b"\r"
        );
        assert_eq!(
            encode_key(&k(KeyCode::Char('한'), KeyModifiers::NONE), false),
            "한".as_bytes()
        );
    }

    #[test]
    fn specs() {
        let s = to_spec(&k(KeyCode::Char('b'), KeyModifiers::CONTROL), false).unwrap();
        assert_eq!(s, KeySpec::parse("ctrl+b").unwrap());
        let s = to_spec(&k(KeyCode::Char('N'), KeyModifiers::SHIFT), true).unwrap();
        assert_eq!(s, KeySpec::parse("prefix+shift+n").unwrap());
        let s = to_spec(&k(KeyCode::Char('?'), KeyModifiers::SHIFT), true).unwrap();
        assert_eq!(s, KeySpec::parse("prefix+question").unwrap());
        let s = to_spec(&k(KeyCode::Char('-'), KeyModifiers::NONE), true).unwrap();
        assert_eq!(s, KeySpec::parse("prefix+minus").unwrap());
    }

    #[test]
    fn paste() {
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        assert_eq!(encode_paste("x", true), b"\x1b[200~x\x1b[201~");
    }
}
