//! 터미널 출력 파서(`kmttt-term`). 창 안의 에이전트는 출력 바이트를 마음대로 만들 수 있다.
//!
//! 확인하는 것:
//! - 어떤 출력·청크 분할·크기 변경에도 패닉하지 않는다(패닉하면 서버가 죽어 모든 창이 끊긴다).
//! - 서버가 PTY 입력으로 되돌려 쓰는 답은 정해진 세 가지 모양뿐이다. 다른 바이트가 섞이면
//!   출력만으로 셸에 글자를 입력할 수 있다는 뜻이다(입력 주입).
#![no_main]

use kmttt_term::Screen;
use libfuzzer_sys::fuzz_target;

fn reply_is_well_formed(mut r: &[u8]) -> bool {
    while !r.is_empty() {
        if let Some(rest) = r.strip_prefix(b"\x1b[0n".as_slice()) {
            r = rest;
        } else if let Some(rest) = r.strip_prefix(b"\x1b[?1;2c".as_slice()) {
            r = rest;
        } else if let Some(rest) = r.strip_prefix(b"\x1b[".as_slice()) {
            // ESC [ 행 ; 열 R — 숫자만 허용
            let end = match rest.iter().position(|&b| b == b'R') {
                Some(e) => e,
                None => return false,
            };
            let body = &rest[..end];
            let mut parts = body.split(|&b| b == b';');
            let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
                return false;
            };
            if a.is_empty() || b.is_empty() || !a.iter().chain(b).all(u8::is_ascii_digit) {
                return false;
            }
            r = &rest[end + 1..];
        } else {
            return false;
        }
    }
    true
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let rows = (data[0] % 60) as u16 + 1;
    let cols = (data[1] % 200) as u16 + 1;
    let chunk = (data[2] as usize % 64) + 1;
    let mut s = Screen::new(rows, cols);
    for (n, piece) in data[3..].chunks(chunk).enumerate() {
        let reply = s.feed_and_reply(piece);
        assert!(
            reply_is_well_formed(&reply),
            "unexpected reply bytes: {reply:?}"
        );
        if n % 17 == 16 {
            let r = (piece[0] % 60) as u16 + 1;
            let c = (piece[piece.len() - 1] % 200) as u16 + 1;
            s.resize(r, c);
        }
    }
    let _ = s.visible_lines();
    let _ = s.tail_lines(5);
    let _ = s.last_line();
    let _ = s.title();
    // 새로 붙은 클라이언트에 보내는 화면 재현 바이트도 다시 해석할 수 있어야 한다.
    let (r, c) = s.size();
    let mut copy = Screen::new(r, c);
    copy.feed(&s.bootstrap_bytes());
});
