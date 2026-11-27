//! 터미널 화면 상태.
//!
//! `Screen`은 VT 시퀀스를 해석해 현재 화면과 스크롤백을 들고 있고,
//! `TextLog`는 제어 문자를 걷어낸 "줄 단위 기록"을 들고 있다(`pane.read --source recent`).

use std::collections::VecDeque;
use std::num::NonZeroU16;

pub use vt100;

/// 스크롤백 줄 수.
pub const SCROLLBACK: usize = 5000;

fn nz(v: u16) -> NonZeroU16 {
    NonZeroU16::new(v.max(1)).expect("non-zero")
}

/// VT 파서 + 화면 버퍼.
pub struct Screen {
    parser: vt100::Parser,
    title: String,
    /// 청크 경계에 걸친 미완성 질의(ESC [ …)를 다음 청크까지 들고 있는 곳
    query_tail: Vec<u8>,
}

impl Screen {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(nz(rows), nz(cols), SCROLLBACK),
            title: String::new(),
            query_tail: Vec::new(),
        }
    }

    /// PTY 출력 바이트를 해석한다. OSC 0/2 창 제목도 여기서 뽑는다.
    pub fn feed(&mut self, bytes: &[u8]) {
        if let Some(t) = extract_title(bytes) {
            self.title = t;
        }
        self.parser.process(bytes);
    }

    /// 출력을 해석하고, 프로그램이 터미널에 물은 질의에 대한 답(PTY로 되돌려 쓸 바이트)을 만든다.
    ///
    /// Windows ConPTY는 시작할 때 커서 위치(ESC [ 6 n)를 묻고 답을 받을 때까지 출력을 멈춘다.
    /// 화면을 가진 쪽은 서버이므로 서버가 답해야 한다. 다른 OS의 프로그램도 같은 질의를 쓴다.
    pub fn feed_and_reply(&mut self, bytes: &[u8]) -> Vec<u8> {
        const QUERIES: [&[u8]; 4] = [b"\x1b[6n", b"\x1b[5n", b"\x1b[0c", b"\x1b[c"];
        let mut buf = std::mem::take(&mut self.query_tail);
        buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut fed = 0; // 아직 해석하지 않은 첫 바이트
        let mut i = 0;
        while i < buf.len() {
            if buf[i] != 0x1b {
                i += 1;
                continue;
            }
            let rest = &buf[i..];
            if let Some(q) = QUERIES.iter().find(|q| rest.starts_with(q)) {
                // 질의까지 먼저 해석해야 커서 위치가 맞다.
                self.feed(&buf[fed..i + q.len()]);
                fed = i + q.len();
                match q[2] {
                    b'6' => {
                        let (row, col) = self.parser.screen().cursor_position();
                        out.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
                    }
                    b'5' => out.extend_from_slice(b"\x1b[0n"),
                    _ => out.extend_from_slice(b"\x1b[?1;2c"),
                }
                i = fed;
                continue;
            }
            if QUERIES
                .iter()
                .any(|q| rest.len() < q.len() && q.starts_with(rest))
            {
                // 청크 끝에서 잘린 질의일 수 있다 → 다음 청크와 합쳐 본다.
                self.feed(&buf[fed..i]);
                self.query_tail = rest.to_vec();
                return out;
            }
            i += 1;
        }
        self.feed(&buf[fed..]);
        out
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(nz(rows), nz(cols));
    }

    pub fn size(&self) -> (u16, u16) {
        let (r, c) = self.parser.screen().size();
        (r.get(), c.get())
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// 화면에 보이는 줄들(오른쪽 공백 제거).
    pub fn visible_lines(&self) -> Vec<String> {
        let (_, cols) = self.size();
        self.parser
            .screen()
            .rows(0, cols)
            .map(|l| l.trim_end().to_string())
            .collect()
    }

    /// 화면 아래쪽에서부터 내용이 있는 마지막 `n`줄. 상태 감지에 쓴다.
    pub fn tail_lines(&self, n: usize) -> Vec<String> {
        let lines = self.visible_lines();
        let last = lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);
        let start = last.saturating_sub(n);
        lines[start..last].to_vec()
    }

    /// 마지막으로 내용이 있는 줄 하나.
    pub fn last_line(&self) -> String {
        self.tail_lines(1).pop().unwrap_or_default()
    }

    /// 새로 붙는 클라이언트가 같은 화면을 재현할 수 있는 바이트열.
    pub fn bootstrap_bytes(&self) -> Vec<u8> {
        let s = self.parser.screen();
        let mut out = s.state_formatted().into_bytes();
        out.extend_from_slice(&s.input_mode_formatted().into_bytes());
        out
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }
}

/// OSC 0/2(창 제목) 시퀀스가 있으면 마지막 것을 꺼낸다.
fn extract_title(bytes: &[u8]) -> Option<String> {
    let mut title = None;
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == 0x1b
            && bytes[i + 1] == b']'
            && (bytes[i + 2] == b'0' || bytes[i + 2] == b'2')
            && bytes[i + 3] == b';'
        {
            let start = i + 4;
            let mut j = start;
            while j < bytes.len()
                && bytes[j] != 0x07
                && !(bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\'))
            {
                j += 1;
            }
            if j < bytes.len() {
                title = Some(
                    String::from_utf8_lossy(&bytes[start..j])
                        .chars()
                        .take(120)
                        .collect(),
                );
            }
            i = j;
        }
        i += 1;
    }
    title
}

/// 제어 문자를 걷어낸 줄 단위 출력 기록. 크기가 정해진 고리 버퍼.
pub struct TextLog {
    lines: VecDeque<String>,
    partial: String,
    max: usize,
    /// 줄 끝 없이 온 `\r`(스피너 등이 같은 줄을 다시 그릴 때). 다음 글자가 `\n`이 아니면 줄을 비운다.
    pending_cr: bool,
}

impl TextLog {
    pub fn new(max: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            partial: String::new(),
            max,
            pending_cr: false,
        }
    }

    /// 이미 ANSI가 제거된 텍스트를 덧붙인다.
    pub fn push_text(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\r' {
                self.pending_cr = true;
                continue;
            }
            if std::mem::take(&mut self.pending_cr) && ch != '\n' {
                self.partial.clear();
            }
            if ch == '\n' {
                let line = std::mem::take(&mut self.partial);
                self.lines.push_back(line);
                while self.lines.len() > self.max {
                    self.lines.pop_front();
                }
            } else if self.partial.len() < 4096 {
                self.partial.push(ch);
            }
        }
    }

    /// 마지막 `n`줄(아직 끝나지 않은 줄 포함).
    pub fn recent(&self, n: usize) -> Vec<String> {
        let mut out: Vec<String> = self.lines.iter().rev().take(n).cloned().collect();
        out.reverse();
        if !self.partial.is_empty() {
            out.push(self.partial.clone());
            if out.len() > n {
                out.remove(0);
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.partial.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_renders_text_and_color() {
        let mut s = Screen::new(5, 20);
        s.feed(b"hello\r\n\x1b[31mred\x1b[0m world");
        assert_eq!(s.visible_lines()[0], "hello");
        assert_eq!(s.visible_lines()[1], "red world");
        assert_eq!(s.last_line(), "red world");
        let cell = s.screen().cell(1, 0).unwrap();
        assert_eq!(cell.fgcolor(), vt100::Color::Idx(1));
    }

    #[test]
    fn wide_korean_characters() {
        let mut s = Screen::new(3, 10);
        s.feed("한글abc".as_bytes());
        assert_eq!(s.visible_lines()[0], "한글abc");
        assert!(s.screen().cell(0, 0).unwrap().is_wide());
        assert!(s.screen().cell(0, 1).unwrap().is_wide_continuation());
    }

    #[test]
    fn resize_keeps_content() {
        let mut s = Screen::new(5, 10);
        s.feed(b"abc");
        s.resize(10, 40);
        assert_eq!(s.size(), (10, 40));
        assert_eq!(s.visible_lines()[0], "abc");
    }

    #[test]
    fn tail_skips_blank_bottom() {
        let mut s = Screen::new(6, 20);
        s.feed(b"a\r\nb\r\nc");
        assert_eq!(s.tail_lines(2), vec!["b", "c"]);
    }

    #[test]
    fn bootstrap_reproduces_screen() {
        let mut a = Screen::new(4, 20);
        a.feed(b"line1\r\n\x1b[1mbold\x1b[0m");
        let mut b = Screen::new(4, 20);
        b.feed(&a.bootstrap_bytes());
        assert_eq!(a.visible_lines(), b.visible_lines());
        assert_eq!(a.screen().cursor_position(), b.screen().cursor_position());
    }

    #[test]
    fn title_from_osc() {
        let mut s = Screen::new(2, 10);
        s.feed(b"\x1b]0;my title\x07x");
        assert_eq!(s.title(), "my title");
    }

    #[test]
    fn textlog_ring() {
        let mut t = TextLog::new(3);
        t.push_text("1\n2\n3\n4\npart");
        assert_eq!(t.recent(10), vec!["2", "3", "4", "part"]);
        assert_eq!(t.recent(2), vec!["4", "part"]);
    }

    #[test]
    fn carriage_return_overwrites_line() {
        let mut t = TextLog::new(10);
        t.push_text("spin 1\rspin 2\r");
        t.push_text("done\r\nnext\n");
        assert_eq!(t.recent(5), vec!["done".to_string(), "next".to_string()]);
    }

    #[test]
    fn answers_terminal_queries() {
        let mut s = Screen::new(10, 40);
        // ConPTY 시작 질의: 커서 위치
        assert_eq!(s.feed_and_reply(b"\x1b[6n"), b"\x1b[1;1R".to_vec());
        // 출력 뒤 위치, 청크 경계에 걸친 질의
        assert!(s.feed_and_reply(b"hello\x1b[").is_empty());
        assert_eq!(s.feed_and_reply(b"6n"), b"\x1b[1;6R".to_vec());
        assert_eq!(s.feed_and_reply(b"\x1b[c"), b"\x1b[?1;2c".to_vec());
        assert_eq!(s.feed_and_reply(b"\x1b[5n"), b"\x1b[0n".to_vec());
        // 질의가 아닌 이스케이프는 그대로 해석
        assert!(s.feed_and_reply(b"\x1b[31mred\x1b[0m").is_empty());
        assert!(s.visible_lines().iter().any(|l| l.contains("hellored")));
    }
}
