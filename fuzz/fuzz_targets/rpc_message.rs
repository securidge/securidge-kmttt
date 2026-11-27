//! 소켓 API의 프레임·메시지 파서(`kmttt-proto`). 창 안의 프로세스도 소켓에 바이트를 보낼 수 있다.
//!
//! 확인하는 것:
//! - 프레임 읽기와 JSON-RPC 해석이 어떤 입력에도 패닉하지 않고, 프레임 크기 상한을 지킨다.
//! - 받아들인 메시지는 다시 직렬화해도 같은 메시지로 해석된다(요청·알림·응답 구분이 바뀌지 않음).
#![no_main]

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use kmttt_proto::{read_frame, RpcMessage, MAX_FRAME};
use libfuzzer_sys::fuzz_target;

/// 메모리 위의 바이트를 읽는 퓨처는 기다리지 않으므로 한 번 폴링하면 끝난다.
fn run<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

fn check_message(body: &[u8]) {
    let Ok(msg) = RpcMessage::from_bytes(body) else {
        return;
    };
    let again = RpcMessage::from_bytes(&msg.to_bytes()).expect("re-encoded message must parse");
    assert_eq!(msg.is_request(), again.is_request());
    assert_eq!(msg.is_notification(), again.is_notification());
    assert_eq!(msg.is_response(), again.is_response());
    assert_eq!(msg.method, again.method);
}

fuzz_target!(|data: &[u8]| {
    // 1) 길이 접두 프레임 스트림으로 읽기
    let mut r = data;
    while let Ok(Some(frame)) = run(read_frame(&mut r)) {
        assert!(frame.len() <= MAX_FRAME);
        check_message(&frame);
    }
    // 2) 본문 하나로 직접 해석
    check_message(data);
});
