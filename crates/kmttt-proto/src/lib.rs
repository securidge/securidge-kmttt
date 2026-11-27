//! KMttt 클라이언트와 서버 사이의 프로토콜.
//!
//! 전송 계층은 "4바이트 빅엔디언 길이 + JSON" 프레임이며, 그 위에 JSON-RPC 2.0 메시지를 싣는다.
//! 같은 프레임이 로컬 소켓, Windows named pipe, SSH 표준 입출력 어디에서나 쓰인다.

pub mod frame;
pub mod schema;
pub mod types;

pub use frame::{read_frame, write_frame, MAX_FRAME};
pub use types::*;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 메시지 하나. 요청·응답·알림을 한 구조로 다룬다.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RpcMessage {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// 오류 코드. -32xxx는 JSON-RPC 표준, -320xx는 KMttt 정의.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    /// 정책 엔진이 거부함.
    pub const POLICY_DENIED: i64 = -32010;
    /// 승인 대기열에서 거부되었거나 시간이 초과됨.
    pub const APPROVAL_DENIED: i64 = -32011;
    /// 대상(pane, 에이전트, 워크스페이스)을 찾지 못함.
    pub const NOT_FOUND: i64 = -32020;
    /// 호출 수 제한 초과.
    pub const RATE_LIMITED: i64 = -32012;
    /// 기다리던 조건이 시간 안에 오지 않음.
    pub const TIMEOUT: i64 = -32030;
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(codes::INVALID_PARAMS, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(codes::NOT_FOUND, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(codes::INTERNAL, message)
    }
}

impl RpcMessage {
    pub fn request(id: u64, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: Some(method.into()),
            params: Some(params),
            ..Default::default()
        }
    }
    pub fn notification(method: &str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: Some(method.into()),
            params: Some(params),
            ..Default::default()
        }
    }
    pub fn response(id: u64, result: Result<Value, RpcError>) -> Self {
        let (result, error) = match result {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        Self {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result,
            error,
            ..Default::default()
        }
    }
    pub fn is_request(&self) -> bool {
        self.method.is_some() && self.id.is_some()
    }
    pub fn is_notification(&self) -> bool {
        self.method.is_some() && self.id.is_none()
    }
    pub fn is_response(&self) -> bool {
        self.method.is_none() && self.id.is_some()
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RpcError> {
        let msg: RpcMessage = serde_json::from_slice(bytes)
            .map_err(|e| RpcError::new(codes::PARSE_ERROR, e.to_string()))?;
        if msg.jsonrpc != "2.0" {
            return Err(RpcError::new(codes::INVALID_REQUEST, "jsonrpc must be 2.0"));
        }
        Ok(msg)
    }
}

/// 바이트열을 JSON에 싣기 위한 base64 도우미.
pub fn b64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn b64_decode(text: &str) -> Result<Vec<u8>, RpcError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|e| RpcError::invalid_params(format!("bad base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_round_trip() {
        let m = RpcMessage::request(7, "pane.read", json!({"pane": "w1:p1"}));
        let back = RpcMessage::from_bytes(&m.to_bytes()).unwrap();
        assert!(back.is_request());
        assert_eq!(back.method.as_deref(), Some("pane.read"));
        assert_eq!(back.id, Some(7));
    }

    #[test]
    fn error_response_round_trip() {
        let m = RpcMessage::response(3, Err(RpcError::not_found("no pane")));
        let back = RpcMessage::from_bytes(&m.to_bytes()).unwrap();
        assert!(back.is_response());
        assert_eq!(back.error.unwrap().code, codes::NOT_FOUND);
    }

    #[test]
    fn rejects_wrong_version() {
        let err = RpcMessage::from_bytes(br#"{"jsonrpc":"1.0","id":1,"method":"x"}"#).unwrap_err();
        assert_eq!(err.code, codes::INVALID_REQUEST);
    }

    #[test]
    fn base64_round_trip() {
        let data = b"\x1b[31mhello\x1b[0m";
        assert_eq!(b64_decode(&b64_encode(data)).unwrap(), data);
    }
}
